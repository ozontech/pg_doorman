#[derive(Copy, Clone, Debug)]
pub(crate) struct CleanupState {
    /// If server connection requires RESET ALL before checkin because of set statement
    pub(crate) needs_cleanup_set: bool,

    /// If server connection requires DEALLOCATE ALL before checkin because of prepare statement
    pub(crate) needs_cleanup_prepare: bool,

    /// If server connection requires CLOSE ALL before checkin because of declare statement
    pub(crate) needs_cleanup_declare: bool,

    /// If server connection requires UNLISTEN * before checkin because of a LISTEN
    /// subscription. Client UNLISTEN does not disarm: the tag is shared by
    /// `UNLISTEN ch` and `UNLISTEN *`, so a partial unsubscribe cannot prove
    /// that no subscription remains.
    pub(crate) needs_cleanup_listen: bool,

    /// If server connection requires DISCARD TEMP before checkin because a
    /// temp-creating tag (CREATE TABLE, CREATE TABLE AS, SELECT INTO) was seen.
    /// The tags do not distinguish temp from permanent objects, so a permanent
    /// CREATE TABLE arms the flag too; DISCARD TEMP is a no-op then.
    pub(crate) needs_cleanup_temp: bool,
}

impl CleanupState {
    pub(crate) fn new() -> Self {
        CleanupState {
            needs_cleanup_set: false,
            needs_cleanup_prepare: false,
            needs_cleanup_declare: false,
            needs_cleanup_listen: false,
            needs_cleanup_temp: false,
        }
    }

    #[inline(always)]
    pub(crate) fn needs_cleanup(&self) -> bool {
        self.needs_cleanup_set
            || self.needs_cleanup_prepare
            || self.needs_cleanup_declare
            || self.needs_cleanup_listen
            || self.needs_cleanup_temp
    }

    #[inline(always)]
    pub(crate) fn set_true(&mut self) {
        self.needs_cleanup_set = true;
        self.needs_cleanup_prepare = true;
        self.needs_cleanup_declare = true;
        self.needs_cleanup_listen = true;
        self.needs_cleanup_temp = true;
    }

    #[inline(always)]
    pub(crate) fn reset(&mut self) {
        self.needs_cleanup_set = false;
        self.needs_cleanup_prepare = false;
        self.needs_cleanup_declare = false;
        self.needs_cleanup_listen = false;
        self.needs_cleanup_temp = false;
    }
}

impl std::fmt::Display for CleanupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SET: {}, PREPARE: {}, DECLARE: {}, LISTEN: {}, TEMP: {}",
            self.needs_cleanup_set,
            self.needs_cleanup_prepare,
            self.needs_cleanup_declare,
            self.needs_cleanup_listen,
            self.needs_cleanup_temp
        )
    }
}
