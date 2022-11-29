// Copyright 2021 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! A rolling window of sessions and cached session info, updated by the state of newly imported blocks.
//!
//! This is useful for consensus components which need to stay up-to-date about recent sessions but don't
//! care about the state of particular blocks.

use super::database::{DBTransaction, Database};
use kvdb::{DBKey, DBOp};

use parity_scale_codec::{Decode, Encode};
pub use polkadot_node_primitives::{new_session_window_size, SessionWindowSize};
use polkadot_primitives::v2::{BlockNumber, Hash, SessionIndex, SessionInfo};
use std::sync::Arc;

use futures::channel::oneshot;
use polkadot_node_subsystem::{
	errors::{ChainApiError, RuntimeApiError},
	messages::{ChainApiMessage, RuntimeApiMessage, RuntimeApiRequest},
	overseer,
};

// The window size is equal to the `approval-voting` and `dispute-coordinator` constants that
// have been obsoleted.
const SESSION_WINDOW_SIZE: SessionWindowSize = new_session_window_size!(6);
const LOG_TARGET: &str = "parachain::rolling-session-window";
const STORED_ROLLING_SESSION_WINDOW: &[u8] = b"Rolling_session_window";

/// Sessions unavailable in state to cache.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SessionsUnavailableReason {
	/// Runtime API subsystem was unavailable.
	#[error(transparent)]
	RuntimeApiUnavailable(#[from] oneshot::Canceled),
	/// The runtime API itself returned an error.
	#[error(transparent)]
	RuntimeApi(#[from] RuntimeApiError),
	/// The chain API itself returned an error.
	#[error(transparent)]
	ChainApi(#[from] ChainApiError),
	/// Missing session info from runtime API for given `SessionIndex`.
	#[error("Missing session index {0:?}")]
	Missing(SessionIndex),
	/// Missing last finalized block number.
	#[error("Missing last finalized block number")]
	MissingLastFinalizedBlock,
	/// Missing last finalized block hash.
	#[error("Missing last finalized block hash")]
	MissingLastFinalizedBlockHash(BlockNumber),
}

/// Information about the sessions being fetched.
#[derive(Debug, Clone)]
pub struct SessionsUnavailableInfo {
	/// The desired window start.
	pub window_start: SessionIndex,
	/// The desired window end.
	pub window_end: SessionIndex,
	/// The block hash whose state the sessions were meant to be drawn from.
	pub block_hash: Hash,
}

/// Sessions were unavailable to fetch from the state for some reason.
#[derive(Debug, thiserror::Error, Clone)]
#[error("Sessions unavailable: {kind:?}, info: {info:?}")]
pub struct SessionsUnavailable {
	/// The error kind.
	#[source]
	kind: SessionsUnavailableReason,
	/// The info about the session window, if any.
	info: Option<SessionsUnavailableInfo>,
}

/// An indicated update of the rolling session window.
#[derive(Debug, PartialEq, Clone)]
pub enum SessionWindowUpdate {
	/// The session window was just advanced from one range to a new one.
	Advanced {
		/// The previous start of the window (inclusive).
		prev_window_start: SessionIndex,
		/// The previous end of the window (inclusive).
		prev_window_end: SessionIndex,
		/// The new start of the window (inclusive).
		new_window_start: SessionIndex,
		/// The new end of the window (inclusive).
		new_window_end: SessionIndex,
	},
	/// The session window was unchanged.
	Unchanged,
}

/// A structure to store rolling session database parameters.
#[derive(Clone)]
pub struct DatabaseParams {
	/// Database reference.
	pub db: Arc<dyn Database>,
	/// The column which stores the rolling session info.
	pub db_column: u32,
}
/// A rolling window of sessions and cached session info.
pub struct RollingSessionWindow {
	earliest_session: SessionIndex,
	session_info: Vec<SessionInfo>,
	window_size: SessionWindowSize,
	// The option is just to enable some approval-voting tests to force feed sessions
	// in the window without dealing with the DB.
	db_params: Option<DatabaseParams>,
}

/// The rolling session data we persist in the database.
#[derive(Encode, Decode, Default)]
struct StoredWindow {
	earliest_session: SessionIndex,
	session_info: Vec<SessionInfo>,
}

impl RollingSessionWindow {
	/// Initialize a new session info cache with the given window size.
	/// Invariant: The database always contains the earliest session. Then,
	/// we can always extend the session info vector using chain state.
	pub async fn new<Sender>(
		mut sender: Sender,
		block_hash: Hash,
		db_params: DatabaseParams,
	) -> Result<Self, SessionsUnavailable>
	where
		Sender: overseer::SubsystemSender<RuntimeApiMessage>
			+ overseer::SubsystemSender<ChainApiMessage>,
	{
		// At first, determine session window start using the chain state.
		let session_index = get_session_index_for_child(&mut sender, block_hash).await?;
		let earliest_non_finalized_block_session =
			Self::earliest_non_finalized_block_session(&mut sender).await?;

		// This will increase the session window to cover the full unfinalized chain.
		let on_chain_window_start = std::cmp::min(
			session_index.saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			earliest_non_finalized_block_session,
		);

		// Fetch session information from DB.
		let maybe_stored_window = Self::db_load(db_params.clone());

		// Get the DB stored sessions and recompute window start based on DB data.
		let (mut window_start, stored_sessions) =
			if let Some(mut stored_window) = maybe_stored_window {
				// Check if DB is ancient.
				if earliest_non_finalized_block_session >
					stored_window.earliest_session + stored_window.session_info.len() as u32
				{
					// If ancient, we scrap it and fetch from chain state.
					stored_window.session_info.clear();
				}

				// The session window might extend beyond the last finalized block, but that's fine as we'll prune it at
				// next update.
				let window_start = if stored_window.session_info.len() > 0 {
					// If there is at least one entry in db, we always take the DB as source of truth.
					stored_window.earliest_session
				} else {
					on_chain_window_start
				};

				(window_start, stored_window.session_info)
			} else {
				(on_chain_window_start, Vec::new())
			};

		// Compute the amount of sessions missing from the window that will be fetched from chain state.
		let sessions_missing_count = session_index
			.saturating_sub(window_start)
			.saturating_add(1)
			.saturating_sub(stored_sessions.len() as u32);

		// Extend from chain state.
		let sessions = if sessions_missing_count > 0 {
			match extend_sessions_from_chain_state(
				stored_sessions,
				&mut sender,
				block_hash,
				&mut window_start,
				session_index,
			)
			.await
			{
				Err(kind) => Err(SessionsUnavailable {
					kind,
					info: Some(SessionsUnavailableInfo {
						window_start,
						window_end: session_index,
						block_hash,
					}),
				}),
				Ok(sessions) => Ok(sessions),
			}?
		} else {
			// There are no new sessions to be fetched from chain state.
			Vec::new()
		};

		Ok(Self {
			earliest_session: window_start,
			session_info: sessions,
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(db_params),
		})
	}

	// Load session information from the parachains db.
	fn db_load(db_params: DatabaseParams) -> Option<StoredWindow> {
		match db_params.db.get(db_params.db_column, STORED_ROLLING_SESSION_WINDOW).ok()? {
			None => None,
			Some(raw) => {
				let maybe_decoded = StoredWindow::decode(&mut &raw[..]).map(Some);
				match maybe_decoded {
					Ok(decoded) => decoded,
					Err(err) => {
						gum::warn!(
							target: LOG_TARGET,
							?err,
							"Failed decoding db entry; will start with onchain session infos and self-heal DB entry on next update."
						);
						None
					},
				}
			},
		}
	}

	// Saves/Updates all sessions in the database.
	// TODO: https://github.com/paritytech/polkadot/issues/6144
	fn db_save(&mut self, stored_window: StoredWindow) {
		if let Some(db_params) = self.db_params.as_ref() {
			match db_params.db.write(DBTransaction {
				ops: vec![DBOp::Insert {
					col: db_params.db_column,
					key: DBKey::from_slice(STORED_ROLLING_SESSION_WINDOW),
					value: stored_window.encode(),
				}],
			}) {
				Ok(_) => {},
				Err(err) => {
					gum::warn!(target: LOG_TARGET, ?err, "Failed writing db entry");
				},
			}
		}
	}

	/// Initialize a new session info cache with the given window size and
	/// initial data.
	/// This is only used in `approval voting` tests.
	pub fn with_session_info(
		earliest_session: SessionIndex,
		session_info: Vec<SessionInfo>,
	) -> Self {
		RollingSessionWindow {
			earliest_session,
			session_info,
			window_size: SESSION_WINDOW_SIZE,
			db_params: None,
		}
	}

	/// Access the session info for the given session index, if stored within the window.
	pub fn session_info(&self, index: SessionIndex) -> Option<&SessionInfo> {
		if index < self.earliest_session {
			None
		} else {
			self.session_info.get((index - self.earliest_session) as usize)
		}
	}

	/// Access the index of the earliest session.
	pub fn earliest_session(&self) -> SessionIndex {
		self.earliest_session
	}

	/// Access the index of the latest session.
	pub fn latest_session(&self) -> SessionIndex {
		self.earliest_session + (self.session_info.len() as SessionIndex).saturating_sub(1)
	}

	/// Returns `true` if `session_index` is contained in the window.
	pub fn contains(&self, session_index: SessionIndex) -> bool {
		session_index >= self.earliest_session() && session_index <= self.latest_session()
	}

	async fn earliest_non_finalized_block_session<Sender>(
		sender: &mut Sender,
	) -> Result<u32, SessionsUnavailable>
	where
		Sender: overseer::SubsystemSender<RuntimeApiMessage>
			+ overseer::SubsystemSender<ChainApiMessage>,
	{
		let last_finalized_height = {
			let (tx, rx) = oneshot::channel();
			sender.send_message(ChainApiMessage::FinalizedBlockNumber(tx)).await;
			match rx.await {
				Ok(Ok(number)) => number,
				Ok(Err(e)) =>
					return Err(SessionsUnavailable {
						kind: SessionsUnavailableReason::ChainApi(e),
						info: None,
					}),
				Err(err) => {
					gum::warn!(
						target: LOG_TARGET,
						?err,
						"Failed fetching last finalized block number"
					);
					return Err(SessionsUnavailable {
						kind: SessionsUnavailableReason::MissingLastFinalizedBlock,
						info: None,
					})
				},
			}
		};

		let (tx, rx) = oneshot::channel();
		// We want to get the session index for the child of the last finalized block.
		sender
			.send_message(ChainApiMessage::FinalizedBlockHash(last_finalized_height, tx))
			.await;
		let last_finalized_hash_parent = match rx.await {
			Ok(Ok(maybe_hash)) => maybe_hash,
			Ok(Err(e)) =>
				return Err(SessionsUnavailable {
					kind: SessionsUnavailableReason::ChainApi(e),
					info: None,
				}),
			Err(err) => {
				gum::warn!(target: LOG_TARGET, ?err, "Failed fetching last finalized block hash");
				return Err(SessionsUnavailable {
					kind: SessionsUnavailableReason::MissingLastFinalizedBlockHash(
						last_finalized_height,
					),
					info: None,
				})
			},
		};

		// Get the session in which the last finalized block was authored.
		if let Some(last_finalized_hash_parent) = last_finalized_hash_parent {
			let session =
				match get_session_index_for_child(sender, last_finalized_hash_parent).await {
					Ok(session_index) => session_index,
					Err(err) => {
						gum::warn!(
							target: LOG_TARGET,
							?err,
							?last_finalized_hash_parent,
							"Failed fetching session index"
						);
						return Err(err)
					},
				};

			Ok(session)
		} else {
			return Err(SessionsUnavailable {
				kind: SessionsUnavailableReason::MissingLastFinalizedBlockHash(
					last_finalized_height,
				),
				info: None,
			})
		}
	}

	/// When inspecting a new import notification, updates the session info cache to match
	/// the session of the imported block's child.
	///
	/// this only needs to be called on heads where we are directly notified about import, as sessions do
	/// not change often and import notifications are expected to be typically increasing in session number.
	///
	/// some backwards drift in session index is acceptable.
	pub async fn cache_session_info_for_head<Sender>(
		&mut self,
		sender: &mut Sender,
		block_hash: Hash,
	) -> Result<SessionWindowUpdate, SessionsUnavailable>
	where
		Sender: overseer::SubsystemSender<RuntimeApiMessage>
			+ overseer::SubsystemSender<ChainApiMessage>,
	{
		let session_index = get_session_index_for_child(sender, block_hash).await?;
		let latest = self.latest_session();

		// Either cached or ancient.
		if session_index <= latest {
			return Ok(SessionWindowUpdate::Unchanged)
		}

		let earliest_non_finalized_block_session =
			Self::earliest_non_finalized_block_session(sender).await?;

		let old_window_start = self.earliest_session;
		let old_window_end = latest;

		// Ensure we keep sessions up to last finalized block by adjusting the window start.
		// This will increase the session window to cover the full unfinalized chain.
		let window_start = std::cmp::min(
			session_index.saturating_sub(self.window_size.get() - 1),
			earliest_non_finalized_block_session,
		);

		// Never look back past earliest session, since if sessions beyond were not needed or available
		// in the past remains valid for the future (window only advances forward).
		let mut window_start = std::cmp::max(window_start, self.earliest_session);

		let mut sessions = self.session_info.clone();
		let sessions_out_of_window = window_start.saturating_sub(old_window_start) as usize;

		let sessions = if sessions_out_of_window < sessions.len() {
			// Drop sessions based on how much the window advanced.
			sessions.split_off((window_start as usize).saturating_sub(old_window_start as usize))
		} else {
			// Window has jumped such that we need to fetch all sessions from on chain.
			Vec::new()
		};

		match extend_sessions_from_chain_state(
			sessions,
			sender,
			block_hash,
			&mut window_start,
			session_index,
		)
		.await
		{
			Err(kind) => Err(SessionsUnavailable {
				kind,
				info: Some(SessionsUnavailableInfo {
					window_start,
					window_end: session_index,
					block_hash,
				}),
			}),
			Ok(s) => {
				let update = SessionWindowUpdate::Advanced {
					prev_window_start: old_window_start,
					prev_window_end: old_window_end,
					new_window_start: window_start,
					new_window_end: session_index,
				};

				self.session_info = s;

				// we need to account for this case:
				// window_start ................................... session_index
				//              old_window_start ........... latest
				let new_earliest = std::cmp::max(window_start, old_window_start);
				self.earliest_session = new_earliest;

				// Update current window in DB.
				self.db_save(StoredWindow {
					earliest_session: self.earliest_session,
					session_info: self.session_info.clone(),
				});
				Ok(update)
			},
		}
	}
}

// Returns the session index expected at any child of the `parent` block.
//
// Note: We could use `RuntimeInfo::get_session_index_for_child` here but it's
// cleaner to just call the runtime API directly without needing to create an instance
// of `RuntimeInfo`.
async fn get_session_index_for_child(
	sender: &mut impl overseer::SubsystemSender<RuntimeApiMessage>,
	block_hash: Hash,
) -> Result<SessionIndex, SessionsUnavailable> {
	let (s_tx, s_rx) = oneshot::channel();

	// We're requesting session index of a child to populate the cache in advance.
	sender
		.send_message(RuntimeApiMessage::Request(
			block_hash,
			RuntimeApiRequest::SessionIndexForChild(s_tx),
		))
		.await;

	match s_rx.await {
		Ok(Ok(s)) => Ok(s),
		Ok(Err(e)) =>
			return Err(SessionsUnavailable {
				kind: SessionsUnavailableReason::RuntimeApi(e),
				info: None,
			}),
		Err(e) =>
			return Err(SessionsUnavailable {
				kind: SessionsUnavailableReason::RuntimeApiUnavailable(e),
				info: None,
			}),
	}
}

/// Attempts to extend db stored sessions with sessions missing between `start` and up to `end_inclusive`.
/// Runtime session info fetching errors are ignored if that doesn't create a gap in the window.
async fn extend_sessions_from_chain_state(
	stored_sessions: Vec<SessionInfo>,
	sender: &mut impl overseer::SubsystemSender<RuntimeApiMessage>,
	block_hash: Hash,
	window_start: &mut SessionIndex,
	end_inclusive: SessionIndex,
) -> Result<Vec<SessionInfo>, SessionsUnavailableReason> {
	// Start from the db sessions.
	let mut sessions = stored_sessions;
	// We allow session fetch failures only if we won't create a gap in the window by doing so.
	// If `allow_failure` is set to true here, fetching errors are ignored until we get a first session.
	let mut allow_failure = sessions.is_empty();

	let start = *window_start + sessions.len() as u32;

	for i in start..=end_inclusive {
		let (tx, rx) = oneshot::channel();
		sender
			.send_message(RuntimeApiMessage::Request(
				block_hash,
				RuntimeApiRequest::SessionInfo(i, tx),
			))
			.await;

		match rx.await {
			Ok(Ok(Some(session_info))) => {
				// We do not allow failure anymore after having at least 1 session in window.
				allow_failure = false;
				sessions.push(session_info);
			},
			Ok(Ok(None)) if !allow_failure => return Err(SessionsUnavailableReason::Missing(i)),
			Ok(Ok(None)) => {
				// Handle `allow_failure` true.
				// If we didn't get the session, we advance window start.
				*window_start += 1;
				gum::debug!(
					target: LOG_TARGET,
					session = ?i,
					"Session info missing from runtime."
				);
			},
			Ok(Err(e)) if !allow_failure => return Err(SessionsUnavailableReason::RuntimeApi(e)),
			Err(canceled) if !allow_failure =>
				return Err(SessionsUnavailableReason::RuntimeApiUnavailable(canceled)),
			Ok(Err(err)) => {
				// Handle `allow_failure` true.
				// If we didn't get the session, we advance window start.
				*window_start += 1;
				gum::debug!(
					target: LOG_TARGET,
					session = ?i,
					?err,
					"Error while fetching session information."
				);
			},
			Err(err) => {
				// Handle `allow_failure` true.
				// If we didn't get the session, we advance window start.
				*window_start += 1;
				gum::debug!(
					target: LOG_TARGET,
					session = ?i,
					?err,
					"Channel error while fetching session information."
				);
			},
		};
	}

	Ok(sessions)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::database::kvdb_impl::DbAdapter;
	use assert_matches::assert_matches;
	use polkadot_node_subsystem::{
		messages::{AllMessages, AvailabilityRecoveryMessage},
		SubsystemContext,
	};
	use polkadot_node_subsystem_test_helpers::make_subsystem_context;
	use polkadot_primitives::v2::Header;
	use sp_core::testing::TaskExecutor;

	const SESSION_DATA_COL: u32 = 0;

	const NUM_COLUMNS: u32 = 1;

	fn dummy_db_params() -> DatabaseParams {
		let db = kvdb_memorydb::create(NUM_COLUMNS);
		let db = DbAdapter::new(db, &[]);
		let db: Arc<dyn Database> = Arc::new(db);
		DatabaseParams { db, db_column: SESSION_DATA_COL }
	}

	fn dummy_session_info(index: SessionIndex) -> SessionInfo {
		SessionInfo {
			validators: Default::default(),
			discovery_keys: Vec::new(),
			assignment_keys: Vec::new(),
			validator_groups: Default::default(),
			n_cores: index as _,
			zeroth_delay_tranche_width: index as _,
			relay_vrf_modulo_samples: index as _,
			n_delay_tranches: index as _,
			no_show_slots: index as _,
			needed_approvals: index as _,
			active_validator_indices: Vec::new(),
			dispute_period: 6,
			random_seed: [0u8; 32],
		}
	}

	fn cache_session_info_test(
		expected_start_session: SessionIndex,
		session: SessionIndex,
		window: Option<RollingSessionWindow>,
		expect_requests_from: SessionIndex,
		db_params: Option<DatabaseParams>,
	) -> RollingSessionWindow {
		let db_params = db_params.unwrap_or(dummy_db_params());

		let header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let finalized_header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) =
			make_subsystem_context::<AvailabilityRecoveryMessage, _>(pool.clone());

		let hash = header.hash();

		let sender = ctx.sender();

		let test_fut = {
			Box::pin(async move {
				let window = match window {
					None =>
						RollingSessionWindow::new(sender.clone(), hash, db_params).await.unwrap(),
					Some(mut window) => {
						window.cache_session_info_for_head(sender, hash).await.unwrap();
						window
					},
				};
				assert_eq!(window.earliest_session, expected_start_session);
				assert_eq!(
					window.session_info,
					(expected_start_session..=session).map(dummy_session_info).collect::<Vec<_>>(),
				);

				window
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
					s_tx,
				)) => {
					let _ = s_tx.send(Ok(finalized_header.number));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
					block_number,
					s_tx,
				)) => {
					assert_eq!(block_number, finalized_header.number);
					let _ = s_tx.send(Ok(Some(finalized_header.hash())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, finalized_header.hash());
					let _ = s_tx.send(Ok(session));
				}
			);

			for i in expect_requests_from..=session {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);
						let _ = s_tx.send(Ok(Some(dummy_session_info(i))));
					}
				);
			}
		});

		let (window, _) = futures::executor::block_on(futures::future::join(test_fut, aux_fut));
		window
	}

	#[test]
	fn cache_session_info_start_empty_db() {
		let db_params = dummy_db_params();

		let window = cache_session_info_test(
			(10 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			10,
			None,
			(10 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			Some(db_params.clone()),
		);

		let window = cache_session_info_test(
			(11 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			11,
			Some(window),
			11,
			None,
		);
		assert_eq!(window.session_info.len(), SESSION_WINDOW_SIZE.get() as usize);

		cache_session_info_test(
			(11 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			12,
			None,
			12,
			Some(db_params),
		);
	}

	#[test]
	fn cache_session_info_first_early() {
		cache_session_info_test(0, 1, None, 0, None);
	}

	#[test]
	fn cache_session_info_does_not_underflow() {
		let window = RollingSessionWindow {
			earliest_session: 1,
			session_info: vec![dummy_session_info(1)],
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		cache_session_info_test(1, 2, Some(window), 2, None);
	}

	#[test]
	fn cache_session_window_contains() {
		let window = RollingSessionWindow {
			earliest_session: 10,
			session_info: vec![dummy_session_info(1)],
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		assert!(!window.contains(0));
		assert!(!window.contains(10 + SESSION_WINDOW_SIZE.get()));
		assert!(!window.contains(11));
		assert!(!window.contains(10 + SESSION_WINDOW_SIZE.get() - 1));
	}

	#[test]
	fn cache_session_info_first_late() {
		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			100,
			None,
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			None,
		);
	}

	#[test]
	fn cache_session_info_jump() {
		let window = RollingSessionWindow {
			earliest_session: 50,
			session_info: vec![
				dummy_session_info(50),
				dummy_session_info(51),
				dummy_session_info(52),
			],
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			100,
			Some(window),
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			None,
		);
	}

	#[test]
	fn cache_session_info_roll_full() {
		let start = 99 - (SESSION_WINDOW_SIZE.get() - 1);
		let window = RollingSessionWindow {
			earliest_session: start,
			session_info: (start..=99).map(dummy_session_info).collect(),
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			100,
			Some(window),
			100, // should only make one request.
			None,
		);
	}

	#[test]
	fn cache_session_info_roll_many_full_db() {
		let db_params = dummy_db_params();
		let start = 97 - (SESSION_WINDOW_SIZE.get() - 1);
		let window = RollingSessionWindow {
			earliest_session: start,
			session_info: (start..=97).map(dummy_session_info).collect(),
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(db_params.clone()),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			100,
			Some(window),
			98,
			None,
		);

		// We expect the session to be populated from DB, and only fetch 101 from on chain.
		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			101,
			None,
			101,
			Some(db_params.clone()),
		);

		// Session warps in the future.
		let window = cache_session_info_test(195, 200, None, 195, Some(db_params));

		assert_eq!(window.session_info.len(), SESSION_WINDOW_SIZE.get() as usize);
	}

	#[test]
	fn cache_session_info_roll_many_full() {
		let start = 97 - (SESSION_WINDOW_SIZE.get() - 1);
		let window = RollingSessionWindow {
			earliest_session: start,
			session_info: (start..=97).map(dummy_session_info).collect(),
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		cache_session_info_test(
			(100 as SessionIndex).saturating_sub(SESSION_WINDOW_SIZE.get() - 1),
			100,
			Some(window),
			98,
			None,
		);
	}

	#[test]
	fn cache_session_info_roll_early() {
		let start = 0;
		let window = RollingSessionWindow {
			earliest_session: start,
			session_info: (0..=1).map(dummy_session_info).collect(),
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		cache_session_info_test(
			0,
			2,
			Some(window),
			2, // should only make one request.
			None,
		);
	}

	#[test]
	fn cache_session_info_roll_many_early() {
		let start = 0;
		let window = RollingSessionWindow {
			earliest_session: start,
			session_info: (0..=1).map(dummy_session_info).collect(),
			window_size: SESSION_WINDOW_SIZE,
			db_params: Some(dummy_db_params()),
		};

		let actual_window_size = window.session_info.len() as u32;

		cache_session_info_test(0, 3, Some(window), actual_window_size, None);
	}

	#[test]
	fn cache_session_fails_for_gap_in_window() {
		// Session index of the tip of our fake test chain.
		let session: SessionIndex = 100;
		let genesis_session: SessionIndex = 0;

		let header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let finalized_header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let hash = header.hash();

		let test_fut = {
			let sender = ctx.sender().clone();
			Box::pin(async move {
				let res = RollingSessionWindow::new(sender, hash, dummy_db_params()).await;

				assert!(res.is_err());
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
					s_tx,
				)) => {
					let _ = s_tx.send(Ok(finalized_header.number));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
					block_number,
					s_tx,
				)) => {
					assert_eq!(block_number, finalized_header.number);
					let _ = s_tx.send(Ok(Some(finalized_header.hash())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, finalized_header.hash());
					let _ = s_tx.send(Ok(0));
				}
			);

			// Unfinalized chain starts at geneisis block, so session 0 is how far we stretch.
			// First 50 sessions are missing.
			for i in genesis_session..=50 {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);
						let _ = s_tx.send(Ok(None));
					}
				);
			}
			// next 10 sessions are present
			for i in 51..=60 {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);
						let _ = s_tx.send(Ok(Some(dummy_session_info(i))));
					}
				);
			}
			// gap of 1 session
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionInfo(j, s_tx),
				)) => {
					assert_eq!(h, hash);
					assert_eq!(61, j);
					let _ = s_tx.send(Ok(None));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn any_session_stretch_with_failure_allowed_for_unfinalized_chain() {
		// Session index of the tip of our fake test chain.
		let session: SessionIndex = 100;
		let genesis_session: SessionIndex = 0;

		let header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let finalized_header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let hash = header.hash();

		let test_fut = {
			let sender = ctx.sender().clone();
			Box::pin(async move {
				let res = RollingSessionWindow::new(sender, hash, dummy_db_params()).await;
				assert!(res.is_ok());
				let rsw = res.unwrap();
				// Since first 50 sessions are missing the earliest should be 50.
				assert_eq!(rsw.earliest_session, 50);
				assert_eq!(rsw.session_info.len(), 51);
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
					s_tx,
				)) => {
					let _ = s_tx.send(Ok(finalized_header.number));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
					block_number,
					s_tx,
				)) => {
					assert_eq!(block_number, finalized_header.number);
					let _ = s_tx.send(Ok(Some(finalized_header.hash())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, finalized_header.hash());
					let _ = s_tx.send(Ok(0));
				}
			);

			// Unfinalized chain starts at geneisis block, so session 0 is how far we stretch.
			// We also test if failure is allowed for 50 first missing sessions.
			for i in genesis_session..=session {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);

						let _ = s_tx.send(Ok(if i < 50 {
							None
						} else {
							Some(dummy_session_info(i))
						}));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn any_session_unavailable_for_caching_means_no_change() {
		let session: SessionIndex = 6;
		let start_session = session.saturating_sub(SESSION_WINDOW_SIZE.get() - 1);

		let header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 5,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let finalized_header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let hash = header.hash();

		let test_fut = {
			let sender = ctx.sender().clone();
			Box::pin(async move {
				let res = RollingSessionWindow::new(sender, hash, dummy_db_params()).await;
				assert!(res.is_err());
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
					s_tx,
				)) => {
					let _ = s_tx.send(Ok(finalized_header.number));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
					block_number,
					s_tx,
				)) => {
					assert_eq!(block_number, finalized_header.number);
					let _ = s_tx.send(Ok(Some(finalized_header.hash())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, finalized_header.hash());
					let _ = s_tx.send(Ok(session));
				}
			);

			for i in start_session..=session {
				assert_matches!(
					handle.recv().await,
					AllMessages::RuntimeApi(RuntimeApiMessage::Request(
						h,
						RuntimeApiRequest::SessionInfo(j, s_tx),
					)) => {
						assert_eq!(h, hash);
						assert_eq!(i, j);

						let _ = s_tx.send(Ok(if i == session {
							None
						} else {
							Some(dummy_session_info(i))
						}));
					}
				);
			}
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}

	#[test]
	fn request_session_info_for_genesis() {
		let session: SessionIndex = 0;

		let header = Header {
			digest: Default::default(),
			extrinsics_root: Default::default(),
			number: 0,
			state_root: Default::default(),
			parent_hash: Default::default(),
		};

		let pool = TaskExecutor::new();
		let (mut ctx, mut handle) = make_subsystem_context::<(), _>(pool.clone());

		let hash = header.hash();

		let test_fut = {
			Box::pin(async move {
				let sender = ctx.sender().clone();
				let window =
					RollingSessionWindow::new(sender, hash, dummy_db_params()).await.unwrap();

				assert_eq!(window.earliest_session, session);
				assert_eq!(window.session_info, vec![dummy_session_info(session)]);
			})
		};

		let aux_fut = Box::pin(async move {
			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, hash);
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockNumber(
					s_tx,
				)) => {
					let _ = s_tx.send(Ok(header.number));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::ChainApi(ChainApiMessage::FinalizedBlockHash(
					block_number,
					s_tx,
				)) => {
					assert_eq!(block_number, header.number);
					let _ = s_tx.send(Ok(Some(header.hash())));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionIndexForChild(s_tx),
				)) => {
					assert_eq!(h, header.hash());
					let _ = s_tx.send(Ok(session));
				}
			);

			assert_matches!(
				handle.recv().await,
				AllMessages::RuntimeApi(RuntimeApiMessage::Request(
					h,
					RuntimeApiRequest::SessionInfo(s, s_tx),
				)) => {
					assert_eq!(h, hash);
					assert_eq!(s, session);

					let _ = s_tx.send(Ok(Some(dummy_session_info(s))));
				}
			);
		});

		futures::executor::block_on(futures::future::join(test_fut, aux_fut));
	}
}
