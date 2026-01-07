#[derive(Copy, Clone, Debug)]
pub(crate) struct CleanupState {
    /// If server connection requires RESET ALL before checkin because of set statement
    pub(crate) needs_cleanup_set: bool,

    /// If server connection requires DEALLOCATE ALL before checkin because of prepare statement
    pub(crate) needs_cleanup_prepare: bool,

    /// If server connection requires CLOSE ALL before checkin because of declare statement
    pub(crate) needs_cleanup_declare: bool,
}

impl CleanupState {
    pub(crate) fn new() -> Self {
        CleanupState {
            needs_cleanup_set: false,
            needs_cleanup_prepare: false,
            needs_cleanup_declare: false,
        }
    }

    #[inline(always)]
    pub(crate) fn needs_cleanup(&self) -> bool {
        self.needs_cleanup_set || self.needs_cleanup_prepare || self.needs_cleanup_declare
    }

    #[inline(always)]
    pub(crate) fn set_true(&mut self) {
        self.needs_cleanup_set = true;
        self.needs_cleanup_prepare = true;
        self.needs_cleanup_declare = true;
    }

    #[inline(always)]
    pub(crate) fn reset(&mut self) {
        self.needs_cleanup_set = false;
        self.needs_cleanup_prepare = false;
        self.needs_cleanup_declare = false;
    }
}

impl std::fmt::Display for CleanupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SET: {}, PREPARE: {}, DECLARE: {}",
            self.needs_cleanup_set, self.needs_cleanup_prepare, self.needs_cleanup_declare
        )
    }
}
