pub(crate) const DEFAULT_PORT: &str = "11300";
pub(crate) const DEFAULT_ADDR: &str = "0.0.0.0";
pub(crate) const MAX_TUBE_NAME_LEN: usize = 200;
pub(crate) const LINE_BUF_SIZE: usize = 11 + 201 + 12;
pub(crate) const JOB_DATA_SIZE_LIMIT_DEFAULT: usize = (1 << 16) - 1;
pub(crate) const JOB_DATA_SIZE_LIMIT_MAX: usize = 1_073_741_824;
pub(crate) const FILE_SIZE_DEFAULT: usize = 10 << 20;
pub(crate) const DEFAULT_FSYNC_MS: u64 = 50;
pub(crate) const URGENT_THRESHOLD: u32 = 1024;
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const NAME_CHARS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-+/;.$_()";
