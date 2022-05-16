use crate::prelude::sp_core;

#[derive(thiserror::Error, Debug)]
pub enum Error {
	#[error("I/O error: `{0}`")]
	Io(#[from] std::io::Error),
	#[error("RPC error: `{0}`")]
	RpcError(#[from] jsonrpsee::core::Error),
	#[error("subxt error: `{0}`")]
	Subxt(#[from] subxt::BasicError),
	#[error("Codec error: `{0}`")]
	Codec(#[from] codec::Error),
	#[error("Crypto error: `{0:?}`")]
	Crypto(sp_core::crypto::SecretStringError),
	#[error("The account doesn't exists")]
	AccountDoesNotExists,
	#[error("Incorrect phase")]
	IncorrectPhase,
	#[error("Submission is already submitted")]
	AlreadySubmitted,
	#[error("Invalid runtime version")]
	VersionMismatch,
	#[error("Submission strategy not satisfied")]
	StrategyNotSatisfied,
	#[error("Submission with better score already exist")]
	BetterScoreExist,
	#[error("Other error: `{0}`")]
	Other(String),
}
