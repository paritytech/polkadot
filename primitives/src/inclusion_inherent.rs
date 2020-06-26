//! Inclusion Inherent primitives define types and constants which can be imported
//! without needing to import the entire inherent module.

use inherents::InherentIdentifier;

/// Unique identifier for the Inclusion Inherent
pub const INHERENT_IDENTIFIER: InherentIdentifier = *b"inclusn0";
