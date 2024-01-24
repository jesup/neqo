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

fn build_coalesce(len: u64) -> RangeTracker {
    let mut used = RangeTracker::default();
    used.mark_range(0, 1000, RangeState::Acked);
    used.mark_range(1000, 100000, RangeState::Sent);
    // leave a gap or it will coalesce here
    for i in 1 .. len {
	// These do not get immediately coalesced when marking since they're not at the end or start
	used.mark_range((i+1) * 1000, 1000, RangeState::Acked); 
    }
    return used;
}

fn coalesce(used: &mut RangeTracker) {
    used.mark_range(1000,1000, RangeState::Acked);
}

fn criterion_benchmark(c: &mut Criterion) {
    c.bench_function("111MB upload", |b| b.iter(|| execute_trace()));
    let mut used = build_coalesce(1);
    c.bench_function("coalesce_acked_from_zero 2 entries", |b| b.iter(|| coalesce(&mut used)));
    used = build_coalesce(100);
    c.bench_function("coalesce_acked_from_zero 100 entries", |b| b.iter(|| coalesce(&mut used)));
    used = build_coalesce(1000);
    c.bench_function("coalesce_acked_from_zero 1000 entries", |b| b.iter(|| coalesce(&mut used)));
}

criterion_group!(benches, criterion_benchmark);
criterion_main!(benches);
