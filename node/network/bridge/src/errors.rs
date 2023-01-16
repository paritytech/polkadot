use polkadot_node_subsystem::SubsystemError;
pub(crate) use polkadot_overseer::OverseerError;

#[fatality::fatality(splitable)]
pub(crate) enum Error {
	/// Received error from overseer:
	#[fatal]
	#[error(transparent)]
	SubsystemError(#[from] SubsystemError),
	/// The stream of incoming events concluded.
	#[fatal]
	#[error("Event stream closed unexpectedly")]
	EventStreamConcluded,
}

impl From<OverseerError> for Error {
	fn from(e: OverseerError) -> Self {
		Error::SubsystemError(SubsystemError::from(e))
	}
}
