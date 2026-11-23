//! Event-key exclusion: the events a caller is moving, which the engine ignores
//! wherever an existing event would otherwise count or block.
//!
//! Rescheduling a booking names it in `Request::exclude_event_keys`, and the
//! engine then reads no block or occurrence with that key as busy time, as `rest`
//! activity or as cap workload. The meeting being moved therefore neither blocks
//! its old slot nor counts against a cap it is moving out of, so a host already
//! at their limit can still be booked elsewhere.
//!
//! The keys are resolved into one sorted set per request, built once and looked up
//! by an event's key. A request that names none pays a single emptiness check, and
//! a request that names many still costs one logarithmic lookup per event rather
//! than a scan of the list.

use std::collections::BTreeSet;

use crate::model::Request;

/// The event keys a request excludes, as one sorted set.
///
/// Not to be read as [`crate::Excluded`], which is a candidate start a search did
/// not offer: this is the caller's own list of the events it is moving.
pub(crate) struct ExcludedKeys<'a> {
    /// The keys, borrowed from the request they came from.
    keys: BTreeSet<&'a str>,
}

impl<'a> ExcludedKeys<'a> {
    /// The event keys `request` excludes.
    pub(crate) fn new(request: &'a Request) -> Self {
        Self {
            keys: request
                .exclude_event_keys
                .iter()
                .map(String::as_str)
                .collect(),
        }
    }

    /// Whether `key` names an excluded event.
    ///
    /// A block with no key is never excluded: an exclusion names an event, and one
    /// the caller cannot identify is not one it can be moving.
    pub(crate) fn excludes(&self, key: Option<&str>) -> bool {
        if self.keys.is_empty() {
            return false;
        }
        key.is_some_and(|key| self.keys.contains(key))
    }
}
