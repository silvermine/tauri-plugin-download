use crate::{ConnectionStatus, Error, Result};

/// On unsupported platforms, returns an [`Error::Unsupported`] error.
pub(crate) fn connection_status() -> Result<ConnectionStatus> {
   Err(Error::Unsupported)
}
