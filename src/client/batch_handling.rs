//! Batch handling for PostgreSQL Extended Query Protocol.
//!
//! This module handles the reordering of ParseComplete messages when some Parse
//! operations are skipped due to prepared statement caching.
//!
//! ## Problem
//!
//! When pg_doorman caches prepared statements, it may skip sending Parse messages
//! to the server for statements that are already cached. However, the client expects
//! to receive ParseComplete responses in the same order as the Parse messages it sent.
//!
//! ## Solution
//!
//! This module tracks batch operations and inserts synthetic ParseComplete messages
//! at the correct positions in the response stream.

use bytes::BytesMut;
use smallvec::SmallVec;

use super::core::{BatchOperation, Client};

/// Type alias for insertion map: stores (index, count) pairs.
/// SmallVec with inline capacity 8 avoids heap allocation for typical batch sizes.
type InsertionMap = SmallVec<[(usize, usize); 8]>;

/// Helper to add or increment count for an index in InsertionMap
#[inline]
fn insertion_map_add(map: &mut InsertionMap, index: usize, count: usize) {
    if let Some(entry) = map.iter_mut().find(|(idx, _)| *idx == index) {
        entry.1 += count;
    } else {
        map.push((index, count));
    }
}

/// Helper to get count for an index from InsertionMap
#[inline]
fn insertion_map_get(map: &InsertionMap, index: usize) -> Option<usize> {
    map.iter()
        .find(|(idx, _)| *idx == index)
        .map(|(_, count)| *count)
}

/// Helper to sum all counts in InsertionMap
#[inline]
fn insertion_map_sum(map: &InsertionMap) -> usize {
    map.iter().map(|(_, count)| *count).sum()
}

// Static ParseComplete message: '1' (1 byte) + length 4 (4 bytes big-endian)
pub(crate) const PARSE_COMPLETE_MSG: [u8; 5] = [b'1', 0, 0, 0, 4];

impl<S, T> Client<S, T>
where
    S: tokio::io::AsyncRead + std::marker::Unpin,
    T: tokio::io::AsyncWrite + std::marker::Unpin,
{
    /// Insert ParseComplete messages into response based on batch_operations order.
    /// This ensures that ParseComplete for skipped Parse operations appears in the
    /// correct position relative to other responses.
    ///
    /// PostgreSQL processes messages in order and sends responses in order:
    /// - Parse → ParseComplete (immediately)
    /// - Bind → BindComplete (immediately)  
    /// - Execute → DataRow + CommandComplete (immediately)
    /// - Describe → ParameterDescription + RowDescription (immediately)
    ///
    /// So for skipped Parse operations, we need to insert ParseComplete at the
    /// ABSOLUTE position in the response stream where the Parse was in the batch.
    ///
    /// This function handles streaming responses - it tracks how many messages have been
    /// processed across multiple chunks using self.processed_response_counts.
    pub(crate) fn reorder_parse_complete_responses(&mut self, response: BytesMut) -> BytesMut {
        if self.prepared.batch_operations.is_empty() || self.prepared.skipped_parses.is_empty() {
            return response;
        }

        // Track which BindComplete/ParameterDescription index needs ParseComplete inserted before it.
        // We can't use absolute positions because Execute returns variable number of messages.
        // Instead, we track the index of BindComplete/ParameterDescription where ParseComplete should go.
        //
        // When ParseSkipped happens, we look at the NEXT operation that will produce a response:
        // - If next is Bind -> insert before that BindComplete
        // - If next is Describe -> insert before that ParameterDescription
        // - If next is Execute/DescribePortal -> we need to insert before the NEXT Bind/Describe after that

        // Maps: BindComplete index -> count of ParseComplete to insert before it
        //       ParameterDescription index -> count of ParseComplete to insert before it
        // Using SmallVec to avoid heap allocation for typical batch sizes (< 8 operations)
        let mut insert_before_bind: InsertionMap = SmallVec::new();
        let mut insert_before_param_desc: InsertionMap = SmallVec::new();

        // Pending ParseComplete insertions waiting for next Bind/Describe
        let mut pending_insertions: usize = 0;

        // Current indices
        let mut bind_index: usize = 0;
        let mut describe_index: usize = 0;

        // Also track Execute index for inserting before Execute's first message
        let mut insert_before_execute: InsertionMap = SmallVec::new();
        let mut execute_index: usize = 0;

        // Track Close index for inserting before CloseComplete
        let mut insert_before_close: InsertionMap = SmallVec::new();
        let mut close_index: usize = 0;

        for op in &self.prepared.batch_operations {
            match op {
                BatchOperation::ParseSkipped { .. } => {
                    // Mark that we need to insert ParseComplete
                    pending_insertions += 1;
                }
                BatchOperation::ParseSent { .. } => {
                    // Server sends ParseComplete, no action needed
                }
                BatchOperation::Describe { .. } => {
                    // Insert pending ParseComplete before this ParameterDescription
                    if pending_insertions > 0 {
                        insertion_map_add(
                            &mut insert_before_param_desc,
                            describe_index,
                            pending_insertions,
                        );
                        pending_insertions = 0;
                    }
                    describe_index += 1;
                }
                BatchOperation::Bind { .. } => {
                    // Insert pending ParseComplete before this BindComplete
                    if pending_insertions > 0 {
                        insertion_map_add(&mut insert_before_bind, bind_index, pending_insertions);
                        pending_insertions = 0;
                    }
                    bind_index += 1;
                }
                BatchOperation::DescribePortal => {
                    // DescribePortal doesn't consume pending insertions
                }
                BatchOperation::Execute => {
                    // Insert pending ParseComplete before this Execute's first message
                    if pending_insertions > 0 {
                        insertion_map_add(
                            &mut insert_before_execute,
                            execute_index,
                            pending_insertions,
                        );
                        pending_insertions = 0;
                    }
                    execute_index += 1;
                }
                BatchOperation::Close => {
                    // Insert pending ParseComplete before this CloseComplete
                    if pending_insertions > 0 {
                        insertion_map_add(
                            &mut insert_before_close,
                            close_index,
                            pending_insertions,
                        );
                        pending_insertions = 0;
                    }
                    close_index += 1;
                }
            }
        }

        // Get offsets from previous chunks
        let bind_offset = self.prepared.processed_response_counts.bind_complete;
        let param_desc_offset = self.prepared.processed_response_counts.param_desc;
        let execute_offset = self.prepared.processed_response_counts.execute;
        let close_offset = self.prepared.processed_response_counts.close_complete;

        // Adjust indices by offset - filter and transform in place to avoid new allocations
        let relevant_bind: InsertionMap = insert_before_bind
            .iter()
            .filter(|(idx, _)| *idx >= bind_offset)
            .map(|(idx, count)| (idx - bind_offset, *count))
            .collect();
        let relevant_param_desc: InsertionMap = insert_before_param_desc
            .iter()
            .filter(|(idx, _)| *idx >= param_desc_offset)
            .map(|(idx, count)| (idx - param_desc_offset, *count))
            .collect();
        let relevant_execute: InsertionMap = insert_before_execute
            .iter()
            .filter(|(idx, _)| *idx >= execute_offset)
            .map(|(idx, count)| (idx - execute_offset, *count))
            .collect();
        let relevant_close: InsertionMap = insert_before_close
            .iter()
            .filter(|(idx, _)| *idx >= close_offset)
            .map(|(idx, count)| (idx - close_offset, *count))
            .collect();

        let total_insertions: usize = insertion_map_sum(&relevant_bind)
            + insertion_map_sum(&relevant_param_desc)
            + insertion_map_sum(&relevant_execute)
            + insertion_map_sum(&relevant_close)
            + pending_insertions; // remaining at end

        if total_insertions == 0 {
            // Still need to count messages for offset tracking
            let mut bind_count = 0usize;
            let mut param_desc_count = 0usize;
            let mut cmd_complete_count = 0usize;
            let mut close_complete_count = 0usize;
            let mut pos = 0;
            while pos + 5 <= response.len() {
                let msg_type = response[pos] as char;
                let msg_len = u32::from_be_bytes([
                    response[pos + 1],
                    response[pos + 2],
                    response[pos + 3],
                    response[pos + 4],
                ]) as usize;
                match msg_type {
                    '2' => bind_count += 1,
                    't' => param_desc_count += 1,
                    'C' => cmd_complete_count += 1,
                    '3' => close_complete_count += 1,
                    _ => {}
                }
                pos += 1 + msg_len;
            }
            self.prepared.processed_response_counts.bind_complete += bind_count;
            self.prepared.processed_response_counts.param_desc += param_desc_count;
            self.prepared.processed_response_counts.execute += cmd_complete_count;
            self.prepared.processed_response_counts.close_complete += close_complete_count;
            return response;
        }

        // Build new response
        let mut new_response = BytesMut::with_capacity(response.len() + total_insertions * 5);
        let mut pos = 0;
        let mut bind_count: usize = 0;
        let mut param_desc_count: usize = 0;
        let mut execute_count: usize = 0;
        let mut close_count: usize = 0;
        let mut in_execute: bool = false; // Track if we're inside an Execute response

        while pos < response.len() {
            if pos + 5 > response.len() {
                new_response.extend_from_slice(&response[pos..]);
                break;
            }

            let msg_type = response[pos] as char;
            let msg_len = u32::from_be_bytes([
                response[pos + 1],
                response[pos + 2],
                response[pos + 3],
                response[pos + 4],
            ]) as usize;

            let msg_end = pos + 1 + msg_len;
            if msg_end > response.len() {
                new_response.extend_from_slice(&response[pos..]);
                break;
            }

            // Insert ParseComplete BEFORE this message if needed
            match msg_type {
                '2' => {
                    if let Some(count) = insertion_map_get(&relevant_bind, bind_count) {
                        for _ in 0..count {
                            new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                        }
                    }
                    bind_count += 1;
                }
                't' => {
                    if let Some(count) = insertion_map_get(&relevant_param_desc, param_desc_count) {
                        for _ in 0..count {
                            new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                        }
                    }
                    param_desc_count += 1;
                }
                '3' => {
                    // CloseComplete - insert pending ParseComplete before it
                    if let Some(count) = insertion_map_get(&relevant_close, close_count) {
                        for _ in 0..count {
                            new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                        }
                    }
                    close_count += 1;
                }
                'C' => {
                    // CommandComplete marks end of Execute
                    in_execute = false;
                    execute_count += 1;
                }
                'D' | 'n' | 'T' => {
                    // DataRow, NoData, or RowDescription can be first message of Execute
                    // Insert ParseComplete before first message of Execute if needed
                    if !in_execute {
                        if let Some(count) = insertion_map_get(&relevant_execute, execute_count) {
                            for _ in 0..count {
                                new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                            }
                        }
                        in_execute = true;
                    }
                }
                'Z' => {
                    // ReadyForQuery - insert any remaining pending ParseComplete before it
                    // This handles the case when batch contains only ParseSkipped + Sync
                    // (without Bind/Describe/Execute)
                    if pending_insertions > 0 {
                        for _ in 0..pending_insertions {
                            new_response.extend_from_slice(&PARSE_COMPLETE_MSG);
                        }
                        pending_insertions = 0;
                    }
                }
                _ => {}
            }

            new_response.extend_from_slice(&response[pos..msg_end]);
            pos = msg_end;
        }

        // Update processed counts
        self.prepared.processed_response_counts.bind_complete += bind_count;
        self.prepared.processed_response_counts.param_desc += param_desc_count;
        self.prepared.processed_response_counts.execute += execute_count;
        self.prepared.processed_response_counts.close_complete += close_count;

        new_response
    }
}
