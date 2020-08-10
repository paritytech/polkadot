
// use polkadot_primitives::v0::{ValidatorId, Hash};

/// Error type for validation
#[derive(Debug, derive_more::Display, derive_more::From)]
pub enum Error {
    // /// Client error
    // Client(sp_blockchain::Error),
    // /// Consensus error
    // Consensus(consensus::error::Error),
    /// An I/O error.
    Io(std::io::Error),

    /// Recieved an invalid approval checker assignment 
    #[display(fmt = "Recieved an invalid approval checker assignment: {}", _
0)]
    BadAssignment(&'static str),
    /// Invalid story used for approval checker assignments
    #[display(fmt = "Invalid story for approval checker assignments: {}", _0)]
    BadStory(&'static str),
}

impl std::error::Error for Error {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                        // Error::Client(ref err) => Some(err),
                        // Error::Consensus(ref err) => Some(err),
                        Error::Io(ref err) => Some(err),
                        _ => None,
                }
        }
}
