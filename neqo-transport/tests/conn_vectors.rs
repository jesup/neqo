// Tests with the test vectors from the spec.
#![deny(warnings)]
use neqo_common::{now, Encoder};
use neqo_crypto::init_db;
use neqo_transport::connection::State;
use neqo_transport::{Connection, Datagram};
use std::net::SocketAddr;

const INITIAL_PACKET: &str = "c1ff000014508394c8f03e51570800449f0dbc195a0000f3a694c75775b4e546172ce9e047cd0b5bee5181648c727adc87f7eae54473ec6cba6bdad4f59823174b769f12358abd292d4f3286934484fb8b239c38732e1f3bbbc6a003056487eb8b5c88b9fd9279ffff3b0f4ecf95c4624db6d65d4113329ee9b0bf8cdd7c8a8d72806d55df25ecb66488bc119d7c9a29abaf99bb33c56b08ad8c26995f838bb3b7a3d5c1858b8ec06b839db2dcf918d5ea9317f1acd6b663cc8925868e2f6a1bda546695f3c3f33175944db4a11a346afb07e78489e509b02add51b7b203eda5c330b03641179a31fbba9b56ce00f3d5b5e3d7d9c5429aebb9576f2f7eacbe27bc1b8082aaf68fb69c921aa5d33ec0c8510410865a178d86d7e54122d55ef2c2bbc040be46d7fece73fe8a1b24495ec160df2da9b20a7ba2f26dfa2a44366dbc63de5cd7d7c94c57172fe6d79c901f025c0010b02c89b395402c009f62dc053b8067a1e0ed0a1e0cf5087d7f78cbd94afe0c3dd55d2d4b1a5cfe2b68b86264e351d1dcd858783a240f893f008ceed743d969b8f735a1677ead960b1fb1ecc5ac83c273b49288d02d7286207e663c45e1a7baf50640c91e762941cf380ce8d79f3e86767fbbcd25b42ef70ec334835a3a6d792e170a432ce0cb7bde9aaa1e75637c1c34ae5fef4338f53db8b13a4d2df594efbfa08784543815c9c0d487bddfa1539bc252cf43ec3686e9802d651cfd2a829a06a9f332a733a4a8aed80efe3478093fbc69c8608146b3f16f1a5c4eac9320da49f1afa5f538ddecbbe7888f435512d0dd74fd9b8c99e3145ba84410d8ca9a36dd884109e76e5fb8222a52e1473da168519ce7a8a3c32e9149671b16724c6c5c51bb5cd64fb591e567fb78b10f9f6fee62c276f282a7df6bcf7c17747bc9a81e6c9c3b032fdd0e1c3ac9eaa5077de3ded18b2ed4faf328f49875af2e36ad5ce5f6cc99ef4b60e57b3b5b9c9fcbcd4cfb3975e70ce4c2506bcd71fef0e53592461504e3d42c885caab21b782e26294c6a9d61118cc40a26f378441ceb48f31a362bf8502a723a36c63502229a462cc2a3796279a5e3a7f81a68c7f81312c381cc16a4ab03513a51ad5b54306ec1d78a5e47e2b15e5b7a1438e5b8b2882dbdad13d6a4a8c3558cae043501b68eb3b040067152337c051c40b5af809aca2856986fd1c86a4ade17d254b6262ac1bc077343b52bf89fa27d73e3c6f3118c9961f0bebe68a5c323c2d84b8c29a2807df663635223242a2ce9828d4429ac270aab5f1841e8e49cf433b1547989f419caa3c758fff96ded40cf3427f0761b678daa1a9e5554465d46b7a917493fc70f9ec5e4e5d786ca501730898aaa1151dcd31829641e29428d90e6065511c24d3109f7cba32225d4accfc54fec42b733f9585252ee36fa5ea0c656934385b468eee245315146b8c047ed27c519b2c0a52d33efe72c186ffe0a230f505676c5324baa6ae006a73e13aa8c39ab173ad2b2778eea0b34c46f2b3beae2c62a2c8db238bf58fc7c27bdceb96c56d29deec87c12351bfd5962497418716a4b915d334ffb5b92ca94ffe1e4f78967042638639a9de325357f5f08f6435061e5a274703936c06fc56af92c420797499ca431a7abaa461863bca656facfad564e6274d4a741033aeeec0ae04c25e7eb50bbaef7183af6bf";

fn loopback() -> SocketAddr {
    "127.0.0.1:443".parse().unwrap()
}

#[test]
fn process_client_initial() {
    init_db("./db");
    let mut server = Connection::new_server(&["key"], &["alpn"]).unwrap();

    let pkt: Vec<u8> = Encoder::from_hex(INITIAL_PACKET).into();
    let dgram = Datagram::new(loopback(), loopback(), pkt);
    assert_eq!(*server.state(), State::WaitInitial);
    let (out, _) = server.process(vec![dgram], now());
    assert_eq!(*server.state(), State::Handshaking);
    assert_eq!(out.len(), 1);
}
