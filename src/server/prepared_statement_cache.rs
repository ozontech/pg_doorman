use log::warn;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::Arc;

use crate::messages::Parse;

// TODO: Add stats the this cache
// TODO: Add application name to the cache value to help identify which application is using the cache
// TODO: Create admin command to show which statements are in the cache
#[derive(Debug)]
pub struct PreparedStatementCache {
    cache: LruCache<u64, Arc<Parse>>,
}

impl PreparedStatementCache {
    pub fn new(mut size: usize) -> Self {
        // Cannot be zeros
        if size == 0 {
            size = 1;
        }

        PreparedStatementCache {
            cache: LruCache::new(NonZeroUsize::new(size).unwrap()),
        }
    }

    /// Adds the prepared statement to the cache if it doesn't exist with a new name
    /// if it already exists will give you the existing parse
    ///
    /// Pass the hash to this so that we can do the compute before acquiring the lock
    pub fn get_or_insert(&mut self, parse: &Parse, hash: u64) -> Arc<Parse> {
        match self.cache.get(&hash) {
            Some(rewritten_parse) => rewritten_parse.clone(),
            None => {
                let new_parse = Arc::new(parse.clone().rewrite());
                let evicted = self.cache.push(hash, new_parse.clone());

                if let Some((_, evicted_parse)) = evicted {
                    warn!(
                        "Evicted prepared statement {} from cache",
                        evicted_parse.name
                    );
                }

                new_parse
            }
        }
    }

    /// Marks the hash as most recently used if it exists
    pub fn promote(&mut self, hash: &u64) {
        self.cache.promote(hash);
    }
}
