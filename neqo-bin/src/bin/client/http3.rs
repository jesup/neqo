// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! An HTTP 3 client implementation.

use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fmt::Display,
    fs::File,
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use neqo_common::{event::Provider, hex, Datagram, Header};
use neqo_crypto::{AuthenticationStatus, ResumptionToken};
use neqo_http3::{Error, Http3Client, Http3ClientEvent, Http3Parameters, Http3State, Priority};
use neqo_transport::{
    AppError, Connection, EmptyConnectionIdGenerator, Error as TransportError, Output, StreamId,
};
use url::Url;

use crate::{get_output_file, qlog_new, Args, KeyUpdateState, Res};

pub(crate) struct Handler<'a> {
    #[allow(
        unknown_lints,
        clippy::struct_field_names,
        clippy::redundant_field_names
    )]
    url_handler: UrlHandler<'a>,
    key_update: KeyUpdateState,
    token: Option<ResumptionToken>,
    output_read_data: bool,
}

impl<'a> Handler<'a> {
    pub(crate) fn new(
        url_queue: VecDeque<Url>,
        args: &'a Args,
        key_update: KeyUpdateState,
    ) -> Self {
        let url_handler = UrlHandler {
            url_queue,
            stream_handlers: HashMap::new(),
            all_paths: Vec::new(),
            handler_type: if args.test.is_some() {
                StreamHandlerType::Upload
            } else {
                StreamHandlerType::Download
            },
            args,
        };

        Self {
            url_handler,
            key_update,
            token: None,
            output_read_data: args.output_read_data,
        }
    }
}

pub(crate) fn create_client(
    args: &Args,
    local_addr: SocketAddr,
    remote_addr: SocketAddr,
    hostname: &str,
    resumption_token: Option<ResumptionToken>,
) -> Res<Http3Client> {
    let mut transport = Connection::new_client(
        hostname,
        &[&args.shared.alpn],
        Rc::new(RefCell::new(EmptyConnectionIdGenerator::default())),
        local_addr,
        remote_addr,
        args.shared.quic_parameters.get(args.shared.alpn.as_str()),
        Instant::now(),
    )?;
    let ciphers = args.get_ciphers();
    if !ciphers.is_empty() {
        transport.set_ciphers(&ciphers)?;
    }
    let mut client = Http3Client::new_with_conn(
        transport,
        Http3Parameters::default()
            .max_table_size_encoder(args.shared.max_table_size_encoder)
            .max_table_size_decoder(args.shared.max_table_size_decoder)
            .max_blocked_streams(args.shared.max_blocked_streams)
            .max_concurrent_push_streams(args.max_concurrent_push_streams),
    );

    let qlog = qlog_new(args, hostname, client.connection_id())?;
    client.set_qlog(qlog);
    if let Some(ech) = &args.ech {
        client.enable_ech(ech).expect("enable ECH");
    }
    if let Some(token) = resumption_token {
        client
            .enable_resumption(Instant::now(), token)
            .expect("enable resumption");
    }

    Ok(client)
}

impl super::Client for Http3Client {
    fn is_closed(&self) -> bool {
        matches!(self.state(), Http3State::Closed(..))
    }

    fn process(&mut self, dgram: Option<&Datagram>, now: Instant) -> Output {
        self.process(dgram, now)
    }

    fn close<S>(&mut self, now: Instant, app_error: AppError, msg: S)
    where
        S: AsRef<str> + Display,
    {
        self.close(now, app_error, msg);
    }
}

impl<'a> super::Handler for Handler<'a> {
    type Client = Http3Client;

    fn handle(&mut self, client: &mut Http3Client) -> Res<bool> {
        while let Some(event) = client.next_event() {
            match event {
                Http3ClientEvent::AuthenticationNeeded => {
                    client.authenticated(AuthenticationStatus::Ok, Instant::now());
                }
                Http3ClientEvent::HeaderReady {
                    stream_id,
                    headers,
                    fin,
                    ..
                } => {
                    if let Some(handler) = self.url_handler.stream_handler(stream_id) {
                        handler.process_header_ready(stream_id, fin, headers);
                    } else {
                        println!("Data on unexpected stream: {stream_id}");
                    }
                    if fin {
                        self.url_handler.on_stream_fin(client, stream_id);
                    }
                }
                Http3ClientEvent::DataReadable { stream_id } => {
                    let mut stream_done = false;
                    match self.url_handler.stream_handler(stream_id) {
                        None => {
                            println!("Data on unexpected stream: {stream_id}");
                        }
                        Some(handler) => loop {
                            let mut data = vec![0; 4096];
                            let (sz, fin) = client
                                .read_data(Instant::now(), stream_id, &mut data)
                                .expect("Read should succeed");

                            handler.process_data_readable(
                                stream_id,
                                fin,
                                data,
                                sz,
                                self.output_read_data,
                            )?;

                            if fin {
                                stream_done = true;
                                break;
                            }

                            if sz == 0 {
                                break;
                            }
                        },
                    }

                    if stream_done {
                        self.url_handler.on_stream_fin(client, stream_id);
                    }
                }
                Http3ClientEvent::DataWritable { stream_id } => {
                    match self.url_handler.stream_handler(stream_id) {
                        None => {
                            println!("Data on unexpected stream: {stream_id}");
                        }
                        Some(handler) => {
                            handler.process_data_writable(client, stream_id);
                        }
                    }
                }
                Http3ClientEvent::StateChange(Http3State::Connected)
                | Http3ClientEvent::RequestsCreatable => {
                    self.url_handler.process_urls(client);
                }
                Http3ClientEvent::ResumptionToken(t) => self.token = Some(t),
                _ => {
                    println!("Unhandled event {event:?}");
                }
            }
        }

        Ok(self.url_handler.done())
    }

    fn maybe_key_update(&mut self, c: &mut Http3Client) -> Res<()> {
        self.key_update.maybe_update(|| c.initiate_key_update())?;
        self.url_handler.process_urls(c);
        Ok(())
    }

    fn take_token(&mut self) -> Option<ResumptionToken> {
        self.token.take()
    }

    fn has_token(&self) -> bool {
        self.token.is_some()
    }
}

trait StreamHandler {
    fn process_header_ready(&mut self, stream_id: StreamId, fin: bool, headers: Vec<Header>);
    fn process_data_readable(
        &mut self,
        stream_id: StreamId,
        fin: bool,
        data: Vec<u8>,
        sz: usize,
        output_read_data: bool,
    ) -> Res<bool>;
    fn process_data_writable(&mut self, client: &mut Http3Client, stream_id: StreamId);
}

enum StreamHandlerType {
    Download,
    Upload,
}

impl StreamHandlerType {
    fn make_handler(
        handler_type: &Self,
        url: &Url,
        args: &Args,
        all_paths: &mut Vec<PathBuf>,
        client: &mut Http3Client,
        client_stream_id: StreamId,
    ) -> Box<dyn StreamHandler> {
        match handler_type {
            Self::Download => {
                let out_file = get_output_file(url, &args.output_dir, all_paths);
                client.stream_close_send(client_stream_id).unwrap();
                Box::new(DownloadStreamHandler { out_file })
            }
            Self::Upload => Box::new(UploadStreamHandler {
                data: vec![42; args.upload_size],
                offset: 0,
                chunk_size: 32768,
                start: Instant::now(),
            }),
        }
    }
}

struct DownloadStreamHandler {
    out_file: Option<File>,
}

impl StreamHandler for DownloadStreamHandler {
    fn process_header_ready(&mut self, stream_id: StreamId, fin: bool, headers: Vec<Header>) {
        if self.out_file.is_none() {
            println!("READ HEADERS[{stream_id}]: fin={fin} {headers:?}");
        }
    }

    fn process_data_readable(
        &mut self,
        stream_id: StreamId,
        fin: bool,
        data: Vec<u8>,
        sz: usize,
        output_read_data: bool,
    ) -> Res<bool> {
        if let Some(out_file) = &mut self.out_file {
            if sz > 0 {
                out_file.write_all(&data[..sz])?;
            }
            return Ok(true);
        } else if !output_read_data {
            println!("READ[{stream_id}]: {sz} bytes");
        } else if let Ok(txt) = String::from_utf8(data.clone()) {
            println!("READ[{stream_id}]: {txt}");
        } else {
            println!("READ[{}]: 0x{}", stream_id, hex(&data));
        }

        if fin && self.out_file.is_none() {
            println!("<FIN[{stream_id}]>");
        }

        Ok(true)
    }

    fn process_data_writable(&mut self, _client: &mut Http3Client, _stream_id: StreamId) {}
}

struct UploadStreamHandler {
    data: Vec<u8>,
    offset: usize,
    chunk_size: usize,
    start: Instant,
}

impl StreamHandler for UploadStreamHandler {
    fn process_header_ready(&mut self, stream_id: StreamId, fin: bool, headers: Vec<Header>) {
        println!("READ HEADERS[{stream_id}]: fin={fin} {headers:?}");
    }

    fn process_data_readable(
        &mut self,
        stream_id: StreamId,
        _fin: bool,
        data: Vec<u8>,
        _sz: usize,
        _output_read_data: bool,
    ) -> Res<bool> {
        if let Ok(txt) = String::from_utf8(data.clone()) {
            let trimmed_txt = txt.trim_end_matches(char::from(0));
            let parsed: usize = trimmed_txt.parse().unwrap();
            if parsed == self.data.len() {
                let upload_time = Instant::now().duration_since(self.start);
                println!("Stream ID: {stream_id:?}, Upload time: {upload_time:?}");
            }
        } else {
            panic!("Unexpected data [{}]: 0x{}", stream_id, hex(&data));
        }
        Ok(true)
    }

    fn process_data_writable(&mut self, client: &mut Http3Client, stream_id: StreamId) {
        while self.offset < self.data.len() {
            let end = self.offset + self.chunk_size.min(self.data.len() - self.offset);
            let chunk = &self.data[self.offset..end];
            match client.send_data(stream_id, chunk) {
                Ok(amount) => {
                    if amount == 0 {
                        break;
                    }
                    self.offset += amount;
                    if self.offset == self.data.len() {
                        client.stream_close_send(stream_id).unwrap();
                    }
                }
                Err(_) => break,
            };
        }
    }
}

struct UrlHandler<'a> {
    url_queue: VecDeque<Url>,
    stream_handlers: HashMap<StreamId, Box<dyn StreamHandler>>,
    all_paths: Vec<PathBuf>,
    handler_type: StreamHandlerType,
    args: &'a Args,
}

impl<'a> UrlHandler<'a> {
    fn stream_handler(&mut self, stream_id: StreamId) -> Option<&mut Box<dyn StreamHandler>> {
        self.stream_handlers.get_mut(&stream_id)
    }

    fn process_urls(&mut self, client: &mut Http3Client) {
        loop {
            if self.url_queue.is_empty() {
                break;
            }
            if self.stream_handlers.len() >= self.args.concurrency {
                break;
            }
            if !self.next_url(client) {
                break;
            }
        }
    }

    fn next_url(&mut self, client: &mut Http3Client) -> bool {
        let url = self
            .url_queue
            .pop_front()
            .expect("download_next called with empty queue");
        match client.fetch(
            Instant::now(),
            &self.args.method,
            &url,
            &to_headers(&self.args.header),
            Priority::default(),
        ) {
            Ok(client_stream_id) => {
                println!("Successfully created stream id {client_stream_id} for {url}");

                let handler: Box<dyn StreamHandler> = StreamHandlerType::make_handler(
                    &self.handler_type,
                    &url,
                    self.args,
                    &mut self.all_paths,
                    client,
                    client_stream_id,
                );
                self.stream_handlers.insert(client_stream_id, handler);
                true
            }
            Err(
                Error::TransportError(TransportError::StreamLimitError)
                | Error::StreamLimitError
                | Error::Unavailable,
            ) => {
                self.url_queue.push_front(url);
                false
            }
            Err(e) => {
                panic!("Can't create stream {e}");
            }
        }
    }

    fn done(&mut self) -> bool {
        self.stream_handlers.is_empty() && self.url_queue.is_empty()
    }

    fn on_stream_fin(&mut self, client: &mut Http3Client, stream_id: StreamId) {
        self.stream_handlers.remove(&stream_id);
        self.process_urls(client);
    }
}

fn to_headers(values: &[impl AsRef<str>]) -> Vec<Header> {
    values
        .iter()
        .scan(None, |state, value| {
            if let Some(name) = state.take() {
                *state = None;
                Some(Header::new(name, value.as_ref()))
            } else {
                *state = Some(value.as_ref().to_string());
                None
            }
        })
        .collect()
}
