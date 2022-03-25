// Copyright 2019-2021 Parity Technologies (UK) Ltd.
// This file is part of Parity Bridges Common.

// Parity Bridges Common is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Parity Bridges Common is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Parity Bridges Common.  If not, see <http://www.gnu.org/licenses/>.

use crate::cli::{Balance, TargetConnectionParams, TargetSigningParams};

use codec::{Decode, Encode};
use num_traits::{One, Zero};
use relay_substrate_client::{
	BlockWithJustification, Chain, Client, Error as SubstrateError, HeaderIdOf, HeaderOf,
	SignParam, TransactionSignScheme,
};
use relay_utils::{FailedClient, HeaderId};
use sp_core::Bytes;
use sp_runtime::{
	traits::{Hash, Header as HeaderT},
	transaction_validity::TransactionPriority,
};
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};
use substrate_relay_helper::TransactionParams;

/// Start resubmit transactions process.
#[derive(StructOpt)]
pub struct ResubmitTransactions {
	/// A bridge instance to relay headers for.
	#[structopt(possible_values = RelayChain::VARIANTS, case_insensitive = true)]
	chain: RelayChain,
	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,
	/// Number of blocks we see before considering queued transaction as stalled.
	#[structopt(long, default_value = "5")]
	stalled_blocks: u32,
	/// Tip limit. We'll never submit transaction with larger tip.
	#[structopt(long)]
	tip_limit: Balance,
	/// Tip increase step. We'll be checking updated transaction priority by increasing its tip by
	/// this step.
	#[structopt(long)]
	tip_step: Balance,
	/// Priority selection strategy.
	#[structopt(subcommand)]
	strategy: PrioritySelectionStrategy,
}

/// Chain, which transactions we're going to track && resubmit.
#[derive(Debug, EnumString, EnumVariantNames)]
#[strum(serialize_all = "kebab_case")]
pub enum RelayChain {
	Millau,
	Kusama,
	Polkadot,
}

/// Strategy to use for priority selection.
#[derive(StructOpt, Debug, PartialEq, Eq, Clone, Copy)]
pub enum PrioritySelectionStrategy {
	/// Strategy selects tip that changes transaction priority to be better than priority of
	/// the first transaction of previous block.
	///
	/// It only makes sense to use this strategy for Millau transactions. Millau has transactions
	/// that are close to block limits, so if there are any other queued transactions, 'large'
	/// transaction won't fit the block && will be postponed. To avoid this, we change its priority
	/// to some large value, making it best transaction => it'll be 'mined' first.
	MakeItBestTransaction,
	/// Strategy selects tip that changes transaction priority to be better than priority of
	/// selected queued transaction.
	///
	/// When we first see stalled transaction, we make it better than worst 1/4 of queued
	/// transactions. If it is still stalled, we'll make it better than 1/3 of queued transactions,
	/// ...
	MakeItBetterThanQueuedTransaction,
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			RelayChain::Millau => {
				type Target = relay_millau_client::Millau;
				type TargetSign = relay_millau_client::Millau;

				$generic
			},
			RelayChain::Kusama => {
				type Target = relay_kusama_client::Kusama;
				type TargetSign = relay_kusama_client::Kusama;

				$generic
			},
			RelayChain::Polkadot => {
				type Target = relay_polkadot_client::Polkadot;
				type TargetSign = relay_polkadot_client::Polkadot;

				$generic
			},
		}
	};
}

impl ResubmitTransactions {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.chain, {
			let relay_loop_name = format!("ResubmitTransactions{}", Target::NAME);
			let client = self.target.to_client::<Target>().await?;
			let transaction_params = TransactionParams {
				signer: self.target_sign.to_keypair::<Target>()?,
				mortality: self.target_sign.target_transactions_mortality,
			};

			relay_utils::relay_loop((), client)
				.run(relay_loop_name, move |_, client, _| {
					run_until_connection_lost::<Target, TargetSign>(
						client,
						transaction_params.clone(),
						Context {
							strategy: self.strategy,
							best_header: HeaderOf::<Target>::new(
								Default::default(),
								Default::default(),
								Default::default(),
								Default::default(),
								Default::default(),
							),
							transaction: None,
							resubmitted: 0,
							stalled_for: Zero::zero(),
							stalled_for_limit: self.stalled_blocks as _,
							tip_step: self.tip_step.cast() as _,
							tip_limit: self.tip_limit.cast() as _,
						},
					)
				})
				.await
				.map_err(Into::into)
		})
	}
}

impl PrioritySelectionStrategy {
	/// Select target priority.
	async fn select_target_priority<C: Chain, S: TransactionSignScheme<Chain = C>>(
		&self,
		client: &Client<C>,
		context: &Context<C>,
	) -> Result<Option<TransactionPriority>, SubstrateError> {
		match *self {
			PrioritySelectionStrategy::MakeItBestTransaction =>
				read_previous_block_best_priority::<C, S>(client, context).await,
			PrioritySelectionStrategy::MakeItBetterThanQueuedTransaction =>
				select_priority_from_queue::<C, S>(client, context).await,
		}
	}
}

#[derive(Debug)]
struct Context<C: Chain> {
	/// Priority selection strategy.
	strategy: PrioritySelectionStrategy,
	/// Best known block header.
	best_header: C::Header,
	/// Hash of the (potentially) stalled transaction.
	transaction: Option<C::Hash>,
	/// How many times we have resubmitted this `transaction`?
	resubmitted: u32,
	/// This transaction is in pool for `stalled_for` wakeup intervals.
	stalled_for: C::BlockNumber,
	/// When `stalled_for` reaching this limit, transaction is considered stalled.
	stalled_for_limit: C::BlockNumber,
	/// Tip step interval.
	tip_step: C::Balance,
	/// Maximal tip.
	tip_limit: C::Balance,
}

impl<C: Chain> Context<C> {
	/// Return true if transaction has stalled.
	fn is_stalled(&self) -> bool {
		self.stalled_for >= self.stalled_for_limit
	}

	/// Notice resubmitted transaction.
	fn notice_resubmitted_transaction(mut self, transaction: C::Hash) -> Self {
		self.transaction = Some(transaction);
		self.stalled_for = Zero::zero();
		self.resubmitted += 1;
		self
	}

	/// Notice transaction from the transaction pool.
	fn notice_transaction(mut self, transaction: C::Hash) -> Self {
		if self.transaction == Some(transaction) {
			self.stalled_for += One::one();
		} else {
			self.transaction = Some(transaction);
			self.stalled_for = One::one();
			self.resubmitted = 0;
		}
		self
	}
}

/// Run resubmit transactions loop.
async fn run_until_connection_lost<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: Client<C>,
	transaction_params: TransactionParams<S::AccountKeyPair>,
	mut context: Context<C>,
) -> Result<(), FailedClient> {
	loop {
		async_std::task::sleep(C::AVERAGE_BLOCK_INTERVAL).await;

		let result =
			run_loop_iteration::<C, S>(client.clone(), transaction_params.clone(), context).await;
		context = match result {
			Ok(context) => context,
			Err(error) => {
				log::error!(
					target: "bridge",
					"Resubmit {} transactions loop has failed with error: {:?}",
					C::NAME,
					error,
				);
				return Err(FailedClient::Target)
			},
		};
	}
}

/// Run single loop iteration.
async fn run_loop_iteration<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: Client<C>,
	transaction_params: TransactionParams<S::AccountKeyPair>,
	mut context: Context<C>,
) -> Result<Context<C>, SubstrateError> {
	// correct best header is required for all other actions
	context.best_header = client.best_header().await?;

	// check if there's queued transaction, signed by given author
	let original_transaction =
		match lookup_signer_transaction::<C, S>(&client, &transaction_params.signer).await? {
			Some(original_transaction) => original_transaction,
			None => {
				log::trace!(target: "bridge", "No {} transactions from required signer in the txpool", C::NAME);
				return Ok(context)
			},
		};
	let original_transaction_hash = C::Hasher::hash(&original_transaction.encode());
	let context = context.notice_transaction(original_transaction_hash);

	// if transaction hasn't been mined for `stalled_blocks`, we'll need to resubmit it
	if !context.is_stalled() {
		log::trace!(
			target: "bridge",
			"{} transaction {:?} is not yet stalled ({:?}/{:?})",
			C::NAME,
			context.transaction,
			context.stalled_for,
			context.stalled_for_limit,
		);
		return Ok(context)
	}

	// select priority for updated transaction
	let target_priority =
		match context.strategy.select_target_priority::<C, S>(&client, &context).await? {
			Some(target_priority) => target_priority,
			None => {
				log::trace!(target: "bridge", "Failed to select target priority");
				return Ok(context)
			},
		};

	// update transaction tip
	let (is_updated, updated_transaction) = update_transaction_tip::<C, S>(
		&client,
		&transaction_params,
		HeaderId(*context.best_header.number(), context.best_header.hash()),
		original_transaction,
		context.tip_step,
		context.tip_limit,
		target_priority,
	)
	.await?;

	if !is_updated {
		log::trace!(target: "bridge", "{} transaction tip can not be updated. Reached limit?", C::NAME);
		return Ok(context)
	}

	let updated_transaction = updated_transaction.encode();
	let updated_transaction_hash = C::Hasher::hash(&updated_transaction);
	client.submit_unsigned_extrinsic(Bytes(updated_transaction)).await?;

	log::info!(
		target: "bridge",
		"Replaced {} transaction {} with {} in txpool",
		C::NAME,
		original_transaction_hash,
		updated_transaction_hash,
	);

	Ok(context.notice_resubmitted_transaction(updated_transaction_hash))
}

/// Search transaction pool for transaction, signed by given key pair.
async fn lookup_signer_transaction<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: &Client<C>,
	key_pair: &S::AccountKeyPair,
) -> Result<Option<S::SignedTransaction>, SubstrateError> {
	let pending_transactions = client.pending_extrinsics().await?;
	for pending_transaction in pending_transactions {
		let pending_transaction = S::SignedTransaction::decode(&mut &pending_transaction.0[..])
			.map_err(SubstrateError::ResponseParseFailed)?;
		if !S::is_signed_by(key_pair, &pending_transaction) {
			continue
		}

		return Ok(Some(pending_transaction))
	}

	Ok(None)
}

/// Read priority of best signed transaction of previous block.
async fn read_previous_block_best_priority<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: &Client<C>,
	context: &Context<C>,
) -> Result<Option<TransactionPriority>, SubstrateError> {
	let best_block = client.get_block(Some(context.best_header.hash())).await?;
	let best_transaction = best_block
		.extrinsics()
		.iter()
		.filter_map(|xt| S::SignedTransaction::decode(&mut &xt[..]).ok())
		.find(|xt| S::is_signed(xt));
	match best_transaction {
		Some(best_transaction) => Ok(Some(
			client
				.validate_transaction(*context.best_header.parent_hash(), best_transaction)
				.await??
				.priority,
		)),
		None => Ok(None),
	}
}

/// Select priority of some queued transaction.
async fn select_priority_from_queue<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: &Client<C>,
	context: &Context<C>,
) -> Result<Option<TransactionPriority>, SubstrateError> {
	// select transaction from the queue
	let queued_transactions = client.pending_extrinsics().await?;
	let selected_transaction = match select_transaction_from_queue(queued_transactions, context) {
		Some(selected_transaction) => selected_transaction,
		None => return Ok(None),
	};

	let selected_transaction = S::SignedTransaction::decode(&mut &selected_transaction[..])
		.map_err(SubstrateError::ResponseParseFailed)?;
	let target_priority = client
		.validate_transaction(context.best_header.hash(), selected_transaction)
		.await??
		.priority;
	Ok(Some(target_priority))
}

/// Select transaction with target priority from the vec of queued transactions.
fn select_transaction_from_queue<C: Chain>(
	mut queued_transactions: Vec<Bytes>,
	context: &Context<C>,
) -> Option<Bytes> {
	if queued_transactions.is_empty() {
		return None
	}

	// the more times we resubmit transaction (`context.resubmitted`), the closer we move
	// to the front of the transaction queue
	let total_transactions = queued_transactions.len();
	let resubmitted_factor = context.resubmitted;
	let divisor =
		1usize.saturating_add(1usize.checked_shl(resubmitted_factor).unwrap_or(usize::MAX));
	let transactions_to_skip = total_transactions / divisor;

	Some(
		queued_transactions
			.swap_remove(std::cmp::min(total_transactions - 1, transactions_to_skip)),
	)
}

/// Try to find appropriate tip for transaction so that its priority is larger than given.
async fn update_transaction_tip<C: Chain, S: TransactionSignScheme<Chain = C>>(
	client: &Client<C>,
	transaction_params: &TransactionParams<S::AccountKeyPair>,
	at_block: HeaderIdOf<C>,
	tx: S::SignedTransaction,
	tip_step: C::Balance,
	tip_limit: C::Balance,
	target_priority: TransactionPriority,
) -> Result<(bool, S::SignedTransaction), SubstrateError> {
	let stx = format!("{:?}", tx);
	let mut current_priority = client.validate_transaction(at_block.1, tx.clone()).await??.priority;
	let mut unsigned_tx = S::parse_transaction(tx).ok_or_else(|| {
		SubstrateError::Custom(format!("Failed to parse {} transaction {}", C::NAME, stx,))
	})?;
	let old_tip = unsigned_tx.tip;

	let (spec_version, transaction_version) = client.simple_runtime_version().await?;
	while current_priority < target_priority {
		let next_tip = unsigned_tx.tip + tip_step;
		if next_tip > tip_limit {
			break
		}

		log::trace!(
			target: "bridge",
			"{} transaction priority with tip={:?}: {}. Target priority: {}",
			C::NAME,
			unsigned_tx.tip,
			current_priority,
			target_priority,
		);

		unsigned_tx.tip = next_tip;
		current_priority = client
			.validate_transaction(
				at_block.1,
				S::sign_transaction(SignParam {
					spec_version,
					transaction_version,
					genesis_hash: *client.genesis_hash(),
					signer: transaction_params.signer.clone(),
					era: relay_substrate_client::TransactionEra::immortal(),
					unsigned: unsigned_tx.clone(),
				})?,
			)
			.await??
			.priority;
	}

	log::debug!(
		target: "bridge",
		"{} transaction tip has changed from {:?} to {:?}",
		C::NAME,
		old_tip,
		unsigned_tx.tip,
	);

	Ok((
		old_tip != unsigned_tx.tip,
		S::sign_transaction(SignParam {
			spec_version,
			transaction_version,
			genesis_hash: *client.genesis_hash(),
			signer: transaction_params.signer.clone(),
			era: relay_substrate_client::TransactionEra::new(
				at_block,
				transaction_params.mortality,
			),
			unsigned: unsigned_tx,
		})?,
	))
}

#[cfg(test)]
mod tests {
	use super::*;
	use bp_rialto::Hash;
	use relay_rialto_client::Rialto;

	fn context() -> Context<Rialto> {
		Context {
			strategy: PrioritySelectionStrategy::MakeItBestTransaction,
			best_header: HeaderOf::<Rialto>::new(
				Default::default(),
				Default::default(),
				Default::default(),
				Default::default(),
				Default::default(),
			),
			transaction: None,
			resubmitted: 0,
			stalled_for: Zero::zero(),
			stalled_for_limit: 3,
			tip_step: 100,
			tip_limit: 1000,
		}
	}

	#[test]
	fn context_works() {
		let mut context = context();

		// when transaction is noticed 2/3 times, it isn't stalled
		context = context.notice_transaction(Default::default());
		assert!(!context.is_stalled());
		assert_eq!(context.stalled_for, 1);
		assert_eq!(context.resubmitted, 0);
		context = context.notice_transaction(Default::default());
		assert!(!context.is_stalled());
		assert_eq!(context.stalled_for, 2);
		assert_eq!(context.resubmitted, 0);

		// when transaction is noticed for 3rd time in a row, it is considered stalled
		context = context.notice_transaction(Default::default());
		assert!(context.is_stalled());
		assert_eq!(context.stalled_for, 3);
		assert_eq!(context.resubmitted, 0);

		// and after we resubmit it, we forget previous transaction
		context = context.notice_resubmitted_transaction(Hash::from([1; 32]));
		assert_eq!(context.transaction, Some(Hash::from([1; 32])));
		assert_eq!(context.resubmitted, 1);
		assert_eq!(context.stalled_for, 0);
	}

	#[test]
	fn select_transaction_from_queue_works_with_empty_queue() {
		assert_eq!(select_transaction_from_queue(vec![], &context()), None);
	}

	#[test]
	fn select_transaction_from_queue_works() {
		let mut context = context();
		let queued_transactions = vec![
			Bytes(vec![1]),
			Bytes(vec![2]),
			Bytes(vec![3]),
			Bytes(vec![4]),
			Bytes(vec![5]),
			Bytes(vec![6]),
		];

		// when we resubmit tx for the first time, 1/2 of queue is skipped
		assert_eq!(
			select_transaction_from_queue(queued_transactions.clone(), &context),
			Some(Bytes(vec![4])),
		);

		// when we resubmit tx for the second time, 1/3 of queue is skipped
		context = context.notice_resubmitted_transaction(Hash::from([1; 32]));
		assert_eq!(
			select_transaction_from_queue(queued_transactions.clone(), &context),
			Some(Bytes(vec![3])),
		);

		// when we resubmit tx for the third time, 1/5 of queue is skipped
		context = context.notice_resubmitted_transaction(Hash::from([2; 32]));
		assert_eq!(
			select_transaction_from_queue(queued_transactions.clone(), &context),
			Some(Bytes(vec![2])),
		);

		// when we resubmit tx for the second time, 1/9 of queue is skipped
		context = context.notice_resubmitted_transaction(Hash::from([3; 32]));
		assert_eq!(
			select_transaction_from_queue(queued_transactions, &context),
			Some(Bytes(vec![1])),
		);
	}
}
