// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// Buffering data to send until it is acked.

use btree_slab::BTreeMap;

use std::{
    cell::RefCell,
    cmp::{max, min, Ordering},
    collections::{BTreeMap, VecDeque},
    convert::TryFrom,
    fmt, mem,
    ops::Add,
    rc::Rc,
};

#[derive(Debug, PartialEq, Clone, Copy)]
enum RangeState {
    Sent,
    Acked,
}

// Because there's no Debug trait for btree_slab::BtreeMap, we have to wrap it
#[derive(Default, PartialEq)]
pub struct RangeMap {
    tree: BTreeMap<u64, (u64, RangeState)>,
}

/// Track ranges in the stream as sent or acked. Acked implies sent. Not in a
/// range implies needing-to-be-sent, either initially or as a retransmission.
#[derive(Debug, Default, PartialEq)]
struct RangeTracker {
    // offset, (len, RangeState). Use u64 for len because ranges can exceed 32bits.
    used: RangeMap,
    cached: Option<(u64, Option<u64>)>,
}

// XXX HACK
impl fmt::Debug for RangeMap {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Hi")
    }
}

impl RangeTracker {
    fn highest_offset(&self) -> u64 {
        self.used
            .tree
            .range(..)
            .next_back()
            .map_or(0, |(k, (v, _))| *k + *v)
    }

    fn acked_from_zero(&self) -> u64 {
        self.used
            .tree
            .get(&0)
            .filter(|(_, state)| *state == RangeState::Acked)
            .map_or(0, |(v, _)| *v)
    }

    /// Find the first unmarked range. If all are contiguous, this will return
    /// (highest_offset(), None).
    fn first_unmarked_range(&mut self) -> (u64, Option<u64>) {
        let mut prev_end = 0;
        if self.cached.is_some() {
            return self.cached.expect("");
        }
        for (cur_off, (cur_len, _)) in &self.used.tree {
            if prev_end == *cur_off {
                prev_end = cur_off + cur_len;
            } else {
                self.cached = Some((prev_end, Some(cur_off - prev_end)));
                return self.cached.expect("");
            }
        }
        self.cached = Some((prev_end, None));
        return self.cached.expect("");
    }

    /// Turn one range into a list of subranges that align with existing
    /// ranges.
    /// Check impermissible overlaps in subregions: Sent cannot overwrite Acked.
    //
    // e.g. given N is new and ABC are existing:
    //             NNNNNNNNNNNNNNNN
    //               AAAAA   BBBCCCCC  ...then we want 5 chunks:
    //             1122222333444555
    //
    // but also if we have this:
    //             NNNNNNNNNNNNNNNN
    //           AAAAAAAAAA      BBBB  ...then break existing A and B ranges up:
    //
    //             1111111122222233
    //           aaAAAAAAAA      BBbb
    //
    // Doing all this work up front should make handling each chunk much
    // easier.
    fn chunk_range_on_edges(
        &mut self,
        new_off: u64,
        new_len: u64,
        new_state: RangeState,
    ) -> Vec<(u64, u64, RangeState)> {
        let mut tmp_off = new_off;
        let mut tmp_len = new_len;
        let mut v = Vec::new();

        // cut previous overlapping range if needed
        let prev = self.used.tree.range_mut(..tmp_off).next_back();
        if let Some((prev_off, (prev_len, prev_state))) = prev {
            let prev_state = *prev_state;
            let overlap = (*prev_off + *prev_len).saturating_sub(new_off);
            *prev_len -= overlap;
            if overlap > 0 {
                self.used.tree.insert(new_off, (overlap, prev_state));
            }
        }

        let mut last_existing_remaining = None;
        for (off, (len, state)) in self.used.tree.range(tmp_off..tmp_off + tmp_len) {
            // Create chunk for "overhang" before an existing range
            if tmp_off < *off {
                let sub_len = off - tmp_off;
                v.push((tmp_off, sub_len, new_state));
                tmp_off += sub_len;
                tmp_len -= sub_len;
            }

            // Create chunk to match existing range
            let sub_len = min(*len, tmp_len);
            let remaining_len = len - sub_len;
            if new_state == RangeState::Sent && *state == RangeState::Acked {
                qinfo!(
                    "Attempted to downgrade overlapping range Acked range {}-{} with Sent {}-{}",
                    off,
                    len,
                    new_off,
                    new_len
                );
            } else {
                v.push((tmp_off, sub_len, new_state));
            }
            tmp_off += sub_len;
            tmp_len -= sub_len;

            if remaining_len > 0 {
                last_existing_remaining = Some((*off, sub_len, remaining_len, *state));
            }
        }

        // Maybe break last existing range in two so that a final chunk will
        // have the same length as an existing range entry
        if let Some((off, sub_len, remaining_len, state)) = last_existing_remaining {
            *self.used.tree.get_mut(&off).expect("must be there") = (sub_len, state);
            self.used.tree.insert(off + sub_len, (remaining_len, state));
        }

        // Create final chunk if anything remains of the new range
        if tmp_len > 0 {
            v.push((tmp_off, tmp_len, new_state))
        }

        v
    }

    /// Merge contiguous Acked ranges into the first entry (0). This range may
    /// be dropped from the send buffer.
    fn coalesce_acked_from_zero(&mut self) {
        let acked_range_from_zero = self
            .used
            .tree
            .get_mut(&0)
            .filter(|(_, state)| *state == RangeState::Acked)
            .map(|(len, _)| *len);

        if let Some(len_from_zero) = acked_range_from_zero {
            let mut to_remove = SmallVec::<[_; 8]>::new();

            let mut new_len_from_zero = len_from_zero;

            // See if there's another Acked range entry contiguous to this one
            while let Some((next_len, _)) = self
                .used
                .tree
                .get(&new_len_from_zero)
                .filter(|(_, state)| *state == RangeState::Acked)
            {
                to_remove.push(new_len_from_zero);
                new_len_from_zero += *next_len;
            }

            if len_from_zero != new_len_from_zero {
                self.used.tree.get_mut(&0).expect("must be there").0 = new_len_from_zero;
            }

            for val in to_remove {
                self.used.tree.remove(&val);
            }
        }
    }

    fn mark_range(&mut self, off: u64, len: usize, state: RangeState) {
        if len == 0 {
            qinfo!("mark 0-length range at {}", off);
            return;
        }

        self.cached = None;
        let subranges = self.chunk_range_on_edges(off, len as u64, state);

        for (sub_off, sub_len, sub_state) in subranges {
            self.used.tree.insert(sub_off, (sub_len, sub_state));
        }

        self.coalesce_acked_from_zero()
    }

    fn unmark_range(&mut self, off: u64, len: usize) {
        if len == 0 {
            qdebug!("unmark 0-length range at {}", off);
            return;
        }

        self.cached = None;
        let len = u64::try_from(len).unwrap();
        let end_off = off + len;

        let mut to_remove = SmallVec::<[_; 8]>::new();
        let mut to_add = None;

        // Walk backwards through possibly affected existing ranges
        for (cur_off, (cur_len, cur_state)) in self.used.tree.range_mut(..off + len).rev() {
            // Maybe fixup range preceding the removed range
            if *cur_off < off {
                // Check for overlap
                if *cur_off + *cur_len > off {
                    if *cur_state == RangeState::Acked {
                        qdebug!(
                            "Attempted to unmark Acked range {}-{} with unmark_range {}-{}",
                            cur_off,
                            cur_len,
                            off,
                            off + len
                        );
                    } else {
                        *cur_len = off - cur_off;
                    }
                }
                break;
            }

            if *cur_state == RangeState::Acked {
                qdebug!(
                    "Attempted to unmark Acked range {}-{} with unmark_range {}-{}",
                    cur_off,
                    cur_len,
                    off,
                    off + len
                );
                continue;
            }

            // Add a new range for old subrange extending beyond
            // to-be-unmarked range
            let cur_end_off = cur_off + *cur_len;
            if cur_end_off > end_off {
                let new_cur_off = off + len;
                let new_cur_len = cur_end_off - end_off;
                assert_eq!(to_add, None);
                to_add = Some((new_cur_off, new_cur_len, *cur_state));
            }

            to_remove.push(*cur_off);
        }

        for remove_off in to_remove {
            self.used.tree.remove(&remove_off);
        }

        if let Some((new_cur_off, new_cur_len, cur_state)) = to_add {
            self.used.tree.insert(new_cur_off, (new_cur_len, cur_state));
        }
    }

    /// Unmark all sent ranges.
    pub fn unmark_sent(&mut self) {
        self.unmark_range(0, usize::try_from(self.highest_offset()).unwrap());
    }
}
