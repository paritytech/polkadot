pub use fatality_proc_macro::fatality;
pub use thiserror;

/// Determine the fatality of an error.
pub trait Fatality {
    fn is_fatal() -> bool;
}
