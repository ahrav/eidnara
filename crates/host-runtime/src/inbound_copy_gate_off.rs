//! Without `test-support`, the inbound copy gate passes every copy through.

#[inline(always)]
pub(crate) fn pass(_len: u32) {}
