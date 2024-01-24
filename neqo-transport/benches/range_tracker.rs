use criterion::{criterion_group, criterion_main, Criterion}; // black_box
use neqo_transport::send_stream::{RangeState, RangeTracker};

// compile the trace into a vec that we can iterate through without the overhead of string parsing
// the included file must have braces or equivalent and be self-contained
// {
// include: grep '\*\*\*' logfile | sed 's/^.*\*\*\*//' | cut -d ' ' -f 2 | sort | uniq | sed 's/^\(.*\)/let mut tracker\1 = RangeTracker::default();/'
// include: grep '\*\*\*' logfile | sed 's/^.*\*\*\*//' | sed 's/^ \([0-9]*\) /tracker\1./' | sed 's/Sent/RangeState::Sent/' | sed 's/Acked/RangeState::Acked/'
// }

fn execute_trace() {
    include!("generated/objects.rs");
}

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("111MB upload", |b| b.iter(|| execute_trace()));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
