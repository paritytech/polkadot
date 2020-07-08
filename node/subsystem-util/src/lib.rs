use futures::future::AbortHandle;
use polkadot_primitives::Hash;
use std::{collections::HashMap, ops::{Deref, DerefMut}};

/// JobCanceler aborts all contained abort handles on drop
#[derive(Debug, Default)]
pub struct JobCanceler(HashMap<Hash, AbortHandle>);

impl Drop for JobCanceler {
    fn drop(&mut self) {
        for abort_handle in self.0.values() {
            abort_handle.abort();
        }
    }
}

// JobCanceler is a smart pointer wrapping the contained hashmap;
// it only cares about the wrapped data insofar as it's necessary
// to implement proper Drop behavior. Therefore, it's appropriate
// to impl Deref and DerefMut.
impl Deref for JobCanceler {
    type Target = HashMap<Hash, AbortHandle>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for JobCanceler {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
