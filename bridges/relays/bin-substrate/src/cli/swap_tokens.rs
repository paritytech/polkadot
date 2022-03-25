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

//! Tokens swap using token-swap bridge pallet.

// TokenSwapBalances fields are never directly accessed, but the whole struct is printed
// to show token swap progress
#![allow(dead_code)]

use codec::Encode;
use num_traits::One;
use rand::random;
use structopt::StructOpt;
use strum::{EnumString, EnumVariantNames, VariantNames};

use frame_support::dispatch::GetDispatchInfo;
use relay_substrate_client::{
	AccountIdOf, AccountPublicOf, BalanceOf, BlockNumberOf, CallOf, Chain, ChainWithBalances,
	Client, Error as SubstrateError, HashOf, SignParam, SignatureOf, Subscription,
	TransactionSignScheme, TransactionStatusOf, UnsignedTransaction,
};
use sp_core::{blake2_256, storage::StorageKey, Bytes, Pair, U256};
use sp_runtime::traits::{Convert, Header as HeaderT};

use crate::cli::{
	estimate_fee::ConversionRateOverride, Balance, CliChain, SourceConnectionParams,
	SourceSigningParams, TargetConnectionParams, TargetSigningParams,
};

/// Swap tokens.
#[derive(StructOpt, Debug, PartialEq)]
pub struct SwapTokens {
	/// A bridge instance to use in token swap.
	#[structopt(possible_values = SwapTokensBridge::VARIANTS, case_insensitive = true)]
	bridge: SwapTokensBridge,

	#[structopt(flatten)]
	source: SourceConnectionParams,
	#[structopt(flatten)]
	source_sign: SourceSigningParams,

	#[structopt(flatten)]
	target: TargetConnectionParams,
	#[structopt(flatten)]
	target_sign: TargetSigningParams,

	#[structopt(subcommand)]
	swap_type: TokenSwapType,
	/// Source chain balance that source signer wants to swap.
	#[structopt(long)]
	source_balance: Balance,
	/// Target chain balance that target signer wants to swap.
	#[structopt(long)]
	target_balance: Balance,
	/// A way to override conversion rate from target to source tokens.
	///
	/// If not specified, conversion rate from runtime storage is used. It may be obsolete and
	/// your message won't be relayed.
	#[structopt(long)]
	target_to_source_conversion_rate_override: Option<ConversionRateOverride>,
	/// A way to override conversion rate from source to target tokens.
	///
	/// If not specified, conversion rate from runtime storage is used. It may be obsolete and
	/// your message won't be relayed.
	#[structopt(long)]
	source_to_target_conversion_rate_override: Option<ConversionRateOverride>,
}

/// Token swap type.
#[derive(StructOpt, Debug, PartialEq, Eq, Clone)]
pub enum TokenSwapType {
	/// The `target_sign` is temporary and only have funds for single swap.
	NoLock,
	/// This swap type prevents `source_signer` from restarting the swap after it has been
	/// completed.
	LockUntilBlock {
		/// Number of blocks before the swap expires.
		#[structopt(long)]
		blocks_before_expire: u32,
		/// Unique swap nonce.
		#[structopt(long)]
		swap_nonce: Option<U256>,
	},
}

/// Swap tokens bridge.
#[derive(Debug, EnumString, EnumVariantNames, PartialEq)]
#[strum(serialize_all = "kebab_case")]
pub enum SwapTokensBridge {
	/// Use token-swap pallet deployed at Millau to swap tokens with Rialto.
	MillauToRialto,
}

macro_rules! select_bridge {
	($bridge: expr, $generic: tt) => {
		match $bridge {
			SwapTokensBridge::MillauToRialto => {
				type Source = relay_millau_client::Millau;
				type Target = relay_rialto_client::Rialto;
				const SOURCE_SPEC_VERSION: u32 = millau_runtime::VERSION.spec_version;
				const TARGET_SPEC_VERSION: u32 = rialto_runtime::VERSION.spec_version;

				type FromSwapToThisAccountIdConverter = bp_rialto::AccountIdConverter;

				use bp_millau::{
					derive_account_from_rialto_id as derive_source_account_from_target_account,
					TO_MILLAU_ESTIMATE_MESSAGE_FEE_METHOD as ESTIMATE_TARGET_TO_SOURCE_MESSAGE_FEE_METHOD,
					WITH_RIALTO_TOKEN_SWAP_PALLET_NAME as TOKEN_SWAP_PALLET_NAME,
				};
				use bp_rialto::{
					derive_account_from_millau_id as derive_target_account_from_source_account,
					TO_RIALTO_ESTIMATE_MESSAGE_FEE_METHOD as ESTIMATE_SOURCE_TO_TARGET_MESSAGE_FEE_METHOD,
				};

				const SOURCE_CHAIN_ID: bp_runtime::ChainId = bp_runtime::MILLAU_CHAIN_ID;
				const TARGET_CHAIN_ID: bp_runtime::ChainId = bp_runtime::RIALTO_CHAIN_ID;

				const SOURCE_TO_TARGET_LANE_ID: bp_messages::LaneId = *b"swap";
				const TARGET_TO_SOURCE_LANE_ID: bp_messages::LaneId = [0, 0, 0, 0];

				$generic
			},
		}
	};
}

impl SwapTokens {
	/// Run the command.
	pub async fn run(self) -> anyhow::Result<()> {
		select_bridge!(self.bridge, {
			let source_client = self.source.to_client::<Source>().await?;
			let source_sign = self.source_sign.to_keypair::<Target>()?;
			let target_client = self.target.to_client::<Target>().await?;
			let target_sign = self.target_sign.to_keypair::<Target>()?;
			let target_to_source_conversion_rate_override =
				self.target_to_source_conversion_rate_override;
			let source_to_target_conversion_rate_override =
				self.source_to_target_conversion_rate_override;

			// names of variables in this function are matching names used by the
			// `pallet-bridge-token-swap`

			// prepare token swap intention
			let token_swap = self
				.prepare_token_swap::<Source, Target>(&source_client, &source_sign, &target_sign)
				.await?;

			// group all accounts that will be used later
			let accounts = TokenSwapAccounts {
				source_account_at_bridged_chain: derive_target_account_from_source_account(
					bp_runtime::SourceAccount::Account(
						token_swap.source_account_at_this_chain.clone(),
					),
				),
				target_account_at_this_chain: derive_source_account_from_target_account(
					bp_runtime::SourceAccount::Account(
						token_swap.target_account_at_bridged_chain.clone(),
					),
				),
				source_account_at_this_chain: token_swap.source_account_at_this_chain.clone(),
				target_account_at_bridged_chain: token_swap.target_account_at_bridged_chain.clone(),
				swap_account: FromSwapToThisAccountIdConverter::convert(
					token_swap.using_encoded(blake2_256).into(),
				),
			};

			// account balances are used to demonstrate what's happening :)
			let initial_balances =
				read_account_balances(&accounts, &source_client, &target_client).await?;

			// before calling something that may fail, log what we're trying to do
			log::info!(target: "bridge", "Starting swap: {:?}", token_swap);
			log::info!(target: "bridge", "Swap accounts: {:?}", accounts);
			log::info!(target: "bridge", "Initial account balances: {:?}", initial_balances);

			//
			// Step 1: swap is created
			//

			// prepare `Currency::transfer` call that will happen at the target chain
			let bridged_currency_transfer: CallOf<Target> = pallet_balances::Call::transfer {
				dest: accounts.source_account_at_bridged_chain.clone().into(),
				value: token_swap.target_balance_at_bridged_chain,
			}
			.into();
			let bridged_currency_transfer_weight =
				bridged_currency_transfer.get_dispatch_info().weight;

			// sign message
			let bridged_chain_spec_version = TARGET_SPEC_VERSION;
			let signature_payload = pallet_bridge_dispatch::account_ownership_digest(
				&bridged_currency_transfer,
				&accounts.swap_account,
				&bridged_chain_spec_version,
				SOURCE_CHAIN_ID,
				TARGET_CHAIN_ID,
			);
			let bridged_currency_transfer_signature: SignatureOf<Target> =
				target_sign.sign(&signature_payload).into();

			// prepare `create_swap` call
			let target_public_at_bridged_chain: AccountPublicOf<Target> =
				target_sign.public().into();
			let swap_delivery_and_dispatch_fee =
				crate::cli::estimate_fee::estimate_message_delivery_and_dispatch_fee::<
					Source,
					Target,
					_,
				>(
					&source_client,
					target_to_source_conversion_rate_override.clone(),
					ESTIMATE_SOURCE_TO_TARGET_MESSAGE_FEE_METHOD,
					SOURCE_TO_TARGET_LANE_ID,
					bp_message_dispatch::MessagePayload {
						spec_version: TARGET_SPEC_VERSION,
						weight: bridged_currency_transfer_weight,
						origin: bp_message_dispatch::CallOrigin::TargetAccount(
							accounts.swap_account.clone(),
							target_public_at_bridged_chain.clone(),
							bridged_currency_transfer_signature.clone(),
						),
						dispatch_fee_payment:
							bp_runtime::messages::DispatchFeePayment::AtTargetChain,
						call: bridged_currency_transfer.encode(),
					},
				)
				.await?;
			let create_swap_call: CallOf<Source> = pallet_bridge_token_swap::Call::create_swap {
				swap: token_swap.clone(),
				swap_creation_params: Box::new(bp_token_swap::TokenSwapCreation {
					target_public_at_bridged_chain,
					swap_delivery_and_dispatch_fee,
					bridged_chain_spec_version,
					bridged_currency_transfer: bridged_currency_transfer.encode(),
					bridged_currency_transfer_weight,
					bridged_currency_transfer_signature,
				}),
			}
			.into();

			// start tokens swap
			let source_genesis_hash = *source_client.genesis_hash();
			let create_swap_signer = source_sign.clone();
			let (spec_version, transaction_version) =
				source_client.simple_runtime_version().await?;
			let swap_created_at = wait_until_transaction_is_finalized::<Source>(
				source_client
					.submit_and_watch_signed_extrinsic(
						accounts.source_account_at_this_chain.clone(),
						move |_, transaction_nonce| {
							Ok(Bytes(
								Source::sign_transaction(SignParam {
									spec_version,
									transaction_version,
									genesis_hash: source_genesis_hash,
									signer: create_swap_signer,
									era: relay_substrate_client::TransactionEra::immortal(),
									unsigned: UnsignedTransaction::new(
										create_swap_call.into(),
										transaction_nonce,
									),
								})?
								.encode(),
							))
						},
					)
					.await?,
			)
			.await?;

			// read state of swap after it has been created
			let token_swap_hash = token_swap.hash();
			let token_swap_storage_key = bp_token_swap::storage_keys::pending_swaps_key(
				TOKEN_SWAP_PALLET_NAME,
				token_swap_hash,
			);
			match read_token_swap_state(&source_client, swap_created_at, &token_swap_storage_key)
				.await?
			{
				Some(bp_token_swap::TokenSwapState::Started) => {
					log::info!(target: "bridge", "Swap has been successfully started");
					let intermediate_balances =
						read_account_balances(&accounts, &source_client, &target_client).await?;
					log::info!(target: "bridge", "Intermediate balances: {:?}", intermediate_balances);
				},
				Some(token_swap_state) =>
					return Err(anyhow::format_err!(
						"Fresh token swap has unexpected state: {:?}",
						token_swap_state,
					)),
				None => return Err(anyhow::format_err!("Failed to start token swap")),
			};

			//
			// Step 2: message is being relayed to the target chain and dispathed there
			//

			// wait until message is dispatched at the target chain and dispatch result delivered
			// back to source chain
			let token_swap_state = wait_until_token_swap_state_is_changed(
				&source_client,
				&token_swap_storage_key,
				bp_token_swap::TokenSwapState::Started,
			)
			.await?;
			let is_transfer_succeeded = match token_swap_state {
				Some(bp_token_swap::TokenSwapState::Started) => {
					unreachable!("wait_until_token_swap_state_is_changed only returns if state is not Started; qed",)
				},
				None =>
					return Err(anyhow::format_err!("Fresh token swap has disappeared unexpectedly")),
				Some(bp_token_swap::TokenSwapState::Confirmed) => {
					log::info!(
						target: "bridge",
						"Transfer has been successfully dispatched at the target chain. Swap can be claimed",
					);
					true
				},
				Some(bp_token_swap::TokenSwapState::Failed) => {
					log::info!(
						target: "bridge",
						"Transfer has been dispatched with an error at the target chain. Swap can be canceled",
					);
					false
				},
			};

			// by this time: (1) token swap account has been created and (2) if transfer has been
			// successfully dispatched, both target chain balances have changed
			let intermediate_balances =
				read_account_balances(&accounts, &source_client, &target_client).await?;
			log::info!(target: "bridge", "Intermediate balances: {:?}", intermediate_balances);

			// transfer has been dispatched, but we may need to wait until block where swap can be
			// claimed/canceled
			if let bp_token_swap::TokenSwapType::LockClaimUntilBlock(
				ref last_available_block_number,
				_,
			) = token_swap.swap_type
			{
				wait_until_swap_unlocked(
					&source_client,
					last_available_block_number + BlockNumberOf::<Source>::one(),
				)
				.await?;
			}

			//
			// Step 3: we may now claim or cancel the swap
			//

			if is_transfer_succeeded {
				log::info!(target: "bridge", "Claiming the swap");

				// prepare `claim_swap` message that will be sent over the bridge
				let claim_swap_call: CallOf<Source> =
					pallet_bridge_token_swap::Call::claim_swap { swap: token_swap }.into();
				let claim_swap_message = bp_message_dispatch::MessagePayload {
					spec_version: SOURCE_SPEC_VERSION,
					weight: claim_swap_call.get_dispatch_info().weight,
					origin: bp_message_dispatch::CallOrigin::SourceAccount(
						accounts.target_account_at_bridged_chain.clone(),
					),
					dispatch_fee_payment: bp_runtime::messages::DispatchFeePayment::AtSourceChain,
					call: claim_swap_call.encode(),
				};
				let claim_swap_delivery_and_dispatch_fee =
					crate::cli::estimate_fee::estimate_message_delivery_and_dispatch_fee::<
						Target,
						Source,
						_,
					>(
						&target_client,
						source_to_target_conversion_rate_override.clone(),
						ESTIMATE_TARGET_TO_SOURCE_MESSAGE_FEE_METHOD,
						TARGET_TO_SOURCE_LANE_ID,
						claim_swap_message.clone(),
					)
					.await?;
				let send_message_call: CallOf<Target> =
					pallet_bridge_messages::Call::send_message {
						lane_id: TARGET_TO_SOURCE_LANE_ID,
						payload: claim_swap_message,
						delivery_and_dispatch_fee: claim_swap_delivery_and_dispatch_fee,
					}
					.into();

				// send `claim_swap` message
				let target_genesis_hash = *target_client.genesis_hash();
				let (spec_version, transaction_version) =
					target_client.simple_runtime_version().await?;
				let _ = wait_until_transaction_is_finalized::<Target>(
					target_client
						.submit_and_watch_signed_extrinsic(
							accounts.target_account_at_bridged_chain.clone(),
							move |_, transaction_nonce| {
								Ok(Bytes(
									Target::sign_transaction(SignParam {
										spec_version,
										transaction_version,
										genesis_hash: target_genesis_hash,
										signer: target_sign,
										era: relay_substrate_client::TransactionEra::immortal(),
										unsigned: UnsignedTransaction::new(
											send_message_call.into(),
											transaction_nonce,
										),
									})?
									.encode(),
								))
							},
						)
						.await?,
				)
				.await?;

				// wait until swap state is updated
				let token_swap_state = wait_until_token_swap_state_is_changed(
					&source_client,
					&token_swap_storage_key,
					bp_token_swap::TokenSwapState::Confirmed,
				)
				.await?;
				if token_swap_state != None {
					return Err(anyhow::format_err!(
						"Confirmed token swap state has been changed to {:?} unexpectedly",
						token_swap_state
					))
				}
			} else {
				log::info!(target: "bridge", "Cancelling the swap");
				let cancel_swap_call: CallOf<Source> =
					pallet_bridge_token_swap::Call::cancel_swap { swap: token_swap.clone() }.into();
				let (spec_version, transaction_version) =
					source_client.simple_runtime_version().await?;
				let _ = wait_until_transaction_is_finalized::<Source>(
					source_client
						.submit_and_watch_signed_extrinsic(
							accounts.source_account_at_this_chain.clone(),
							move |_, transaction_nonce| {
								Ok(Bytes(
									Source::sign_transaction(SignParam {
										spec_version,
										transaction_version,
										genesis_hash: source_genesis_hash,
										signer: source_sign,
										era: relay_substrate_client::TransactionEra::immortal(),
										unsigned: UnsignedTransaction::new(
											cancel_swap_call.into(),
											transaction_nonce,
										),
									})?
									.encode(),
								))
							},
						)
						.await?,
				)
				.await?;
			}

			// print final balances
			let final_balances =
				read_account_balances(&accounts, &source_client, &target_client).await?;
			log::info!(target: "bridge", "Final account balances: {:?}", final_balances);

			Ok(())
		})
	}

	/// Prepare token swap intention.
	async fn prepare_token_swap<Source: CliChain, Target: CliChain>(
		&self,
		source_client: &Client<Source>,
		source_sign: &Source::KeyPair,
		target_sign: &Target::KeyPair,
	) -> anyhow::Result<
		bp_token_swap::TokenSwap<
			BlockNumberOf<Source>,
			BalanceOf<Source>,
			AccountIdOf<Source>,
			BalanceOf<Target>,
			AccountIdOf<Target>,
		>,
	>
	where
		AccountIdOf<Source>: From<<Source::KeyPair as Pair>::Public>,
		AccountIdOf<Target>: From<<Target::KeyPair as Pair>::Public>,
		BalanceOf<Source>: From<u64>,
		BalanceOf<Target>: From<u64>,
	{
		// accounts that are directly controlled by participants
		let source_account_at_this_chain: AccountIdOf<Source> = source_sign.public().into();
		let target_account_at_bridged_chain: AccountIdOf<Target> = target_sign.public().into();

		// balances that we're going to swap
		let source_balance_at_this_chain: BalanceOf<Source> = self.source_balance.cast().into();
		let target_balance_at_bridged_chain: BalanceOf<Target> = self.target_balance.cast().into();

		// prepare token swap intention
		Ok(bp_token_swap::TokenSwap {
			swap_type: self.prepare_token_swap_type(source_client).await?,
			source_balance_at_this_chain,
			source_account_at_this_chain: source_account_at_this_chain.clone(),
			target_balance_at_bridged_chain,
			target_account_at_bridged_chain: target_account_at_bridged_chain.clone(),
		})
	}

	/// Prepare token swap type.
	async fn prepare_token_swap_type<Source: Chain>(
		&self,
		source_client: &Client<Source>,
	) -> anyhow::Result<bp_token_swap::TokenSwapType<BlockNumberOf<Source>>> {
		match self.swap_type {
			TokenSwapType::NoLock =>
				Ok(bp_token_swap::TokenSwapType::TemporaryTargetAccountAtBridgedChain),
			TokenSwapType::LockUntilBlock { blocks_before_expire, ref swap_nonce } => {
				let blocks_before_expire: BlockNumberOf<Source> = blocks_before_expire.into();
				let current_source_block_number = *source_client.best_header().await?.number();
				Ok(bp_token_swap::TokenSwapType::LockClaimUntilBlock(
					current_source_block_number + blocks_before_expire,
					swap_nonce.unwrap_or_else(|| {
						U256::from(random::<u128>()).overflowing_mul(U256::from(random::<u128>())).0
					}),
				))
			},
		}
	}
}

/// Accounts that are participating in the swap.
#[derive(Debug)]
struct TokenSwapAccounts<ThisAccountId, BridgedAccountId> {
	source_account_at_this_chain: ThisAccountId,
	source_account_at_bridged_chain: BridgedAccountId,
	target_account_at_bridged_chain: BridgedAccountId,
	target_account_at_this_chain: ThisAccountId,
	swap_account: ThisAccountId,
}

/// Swap accounts balances.
#[derive(Debug)]
struct TokenSwapBalances<ThisBalance, BridgedBalance> {
	source_account_at_this_chain_balance: Option<ThisBalance>,
	source_account_at_bridged_chain_balance: Option<BridgedBalance>,
	target_account_at_bridged_chain_balance: Option<BridgedBalance>,
	target_account_at_this_chain_balance: Option<ThisBalance>,
	swap_account_balance: Option<ThisBalance>,
}

/// Read swap accounts balances.
async fn read_account_balances<Source: ChainWithBalances, Target: ChainWithBalances>(
	accounts: &TokenSwapAccounts<AccountIdOf<Source>, AccountIdOf<Target>>,
	source_client: &Client<Source>,
	target_client: &Client<Target>,
) -> anyhow::Result<TokenSwapBalances<BalanceOf<Source>, BalanceOf<Target>>> {
	Ok(TokenSwapBalances {
		source_account_at_this_chain_balance: read_account_balance(
			source_client,
			&accounts.source_account_at_this_chain,
		)
		.await?,
		source_account_at_bridged_chain_balance: read_account_balance(
			target_client,
			&accounts.source_account_at_bridged_chain,
		)
		.await?,
		target_account_at_bridged_chain_balance: read_account_balance(
			target_client,
			&accounts.target_account_at_bridged_chain,
		)
		.await?,
		target_account_at_this_chain_balance: read_account_balance(
			source_client,
			&accounts.target_account_at_this_chain,
		)
		.await?,
		swap_account_balance: read_account_balance(source_client, &accounts.swap_account).await?,
	})
}

/// Read account balance.
async fn read_account_balance<C: ChainWithBalances>(
	client: &Client<C>,
	account: &AccountIdOf<C>,
) -> anyhow::Result<Option<BalanceOf<C>>> {
	match client.free_native_balance(account.clone()).await {
		Ok(balance) => Ok(Some(balance)),
		Err(SubstrateError::AccountDoesNotExist) => Ok(None),
		Err(error) => Err(anyhow::format_err!(
			"Failed to read balance of {} account {:?}: {:?}",
			C::NAME,
			account,
			error,
		)),
	}
}

/// Wait until transaction is included into finalized block.
///
/// Returns the hash of the finalized block with transaction.
pub(crate) async fn wait_until_transaction_is_finalized<C: Chain>(
	subscription: Subscription<TransactionStatusOf<C>>,
) -> anyhow::Result<HashOf<C>> {
	loop {
		let transaction_status = subscription.next().await?;
		match transaction_status {
			Some(TransactionStatusOf::<C>::FinalityTimeout(_)) |
			Some(TransactionStatusOf::<C>::Usurped(_)) |
			Some(TransactionStatusOf::<C>::Dropped) |
			Some(TransactionStatusOf::<C>::Invalid) |
			None =>
				return Err(anyhow::format_err!(
					"We've been waiting for finalization of {} transaction, but it now has the {:?} status",
					C::NAME,
					transaction_status,
				)),
			Some(TransactionStatusOf::<C>::Finalized(block_hash)) => {
				log::trace!(
					target: "bridge",
					"{} transaction has been finalized at block {}",
					C::NAME,
					block_hash,
				);
				return Ok(block_hash)
			},
			_ => {
				log::trace!(
					target: "bridge",
					"Received intermediate status of {} transaction: {:?}",
					C::NAME,
					transaction_status,
				);
			},
		}
	}
}

/// Waits until token swap state is changed from `Started` to something else.
async fn wait_until_token_swap_state_is_changed<C: Chain>(
	client: &Client<C>,
	swap_state_storage_key: &StorageKey,
	previous_token_swap_state: bp_token_swap::TokenSwapState,
) -> anyhow::Result<Option<bp_token_swap::TokenSwapState>> {
	log::trace!(target: "bridge", "Waiting for token swap state change");
	loop {
		async_std::task::sleep(C::AVERAGE_BLOCK_INTERVAL).await;

		let best_block = client.best_finalized_header_number().await?;
		let best_block_hash = client.block_hash_by_number(best_block).await?;
		log::trace!(target: "bridge", "Inspecting {} block {}/{}", C::NAME, best_block, best_block_hash);

		let token_swap_state =
			read_token_swap_state(client, best_block_hash, swap_state_storage_key).await?;
		match token_swap_state {
			Some(new_token_swap_state) if new_token_swap_state == previous_token_swap_state => {},
			_ => {
				log::trace!(
					target: "bridge",
					"Token swap state has been changed from {:?} to {:?}",
					previous_token_swap_state,
					token_swap_state,
				);
				return Ok(token_swap_state)
			},
		}
	}
}

/// Waits until swap can be claimed or canceled.
async fn wait_until_swap_unlocked<C: Chain>(
	client: &Client<C>,
	required_block_number: BlockNumberOf<C>,
) -> anyhow::Result<()> {
	log::trace!(target: "bridge", "Waiting for token swap unlock");
	loop {
		async_std::task::sleep(C::AVERAGE_BLOCK_INTERVAL).await;

		let best_block = client.best_finalized_header_number().await?;
		let best_block_hash = client.block_hash_by_number(best_block).await?;
		if best_block >= required_block_number {
			return Ok(())
		}

		log::trace!(target: "bridge", "Skipping {} block {}/{}", C::NAME, best_block, best_block_hash);
	}
}

/// Read state of the active token swap.
async fn read_token_swap_state<C: Chain>(
	client: &Client<C>,
	at_block: C::Hash,
	swap_state_storage_key: &StorageKey,
) -> anyhow::Result<Option<bp_token_swap::TokenSwapState>> {
	Ok(client.storage_value(swap_state_storage_key.clone(), Some(at_block)).await?)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::cli::{RuntimeVersionType, SourceRuntimeVersionParams, TargetRuntimeVersionParams};

	#[test]
	fn swap_tokens_millau_to_rialto_no_lock() {
		let swap_tokens = SwapTokens::from_iter(vec![
			"swap-tokens",
			"millau-to-rialto",
			"--source-host",
			"127.0.0.1",
			"--source-port",
			"9000",
			"--source-signer",
			"//Alice",
			"--source-balance",
			"8000000000",
			"--target-host",
			"127.0.0.1",
			"--target-port",
			"9001",
			"--target-signer",
			"//Bob",
			"--target-balance",
			"9000000000",
			"no-lock",
		]);

		assert_eq!(
			swap_tokens,
			SwapTokens {
				bridge: SwapTokensBridge::MillauToRialto,
				source: SourceConnectionParams {
					source_host: "127.0.0.1".into(),
					source_port: 9000,
					source_secure: false,
					source_runtime_version: SourceRuntimeVersionParams {
						source_version_mode: RuntimeVersionType::Bundle,
						source_spec_version: None,
						source_transaction_version: None,
					}
				},
				source_sign: SourceSigningParams {
					source_signer: Some("//Alice".into()),
					source_signer_password: None,
					source_signer_file: None,
					source_signer_password_file: None,
					source_transactions_mortality: None,
				},
				target: TargetConnectionParams {
					target_host: "127.0.0.1".into(),
					target_port: 9001,
					target_secure: false,
					target_runtime_version: TargetRuntimeVersionParams {
						target_version_mode: RuntimeVersionType::Bundle,
						target_spec_version: None,
						target_transaction_version: None,
					}
				},
				target_sign: TargetSigningParams {
					target_signer: Some("//Bob".into()),
					target_signer_password: None,
					target_signer_file: None,
					target_signer_password_file: None,
					target_transactions_mortality: None,
				},
				swap_type: TokenSwapType::NoLock,
				source_balance: Balance(8000000000),
				target_balance: Balance(9000000000),
				target_to_source_conversion_rate_override: None,
				source_to_target_conversion_rate_override: None,
			}
		);
	}

	#[test]
	fn swap_tokens_millau_to_rialto_lock_until() {
		let swap_tokens = SwapTokens::from_iter(vec![
			"swap-tokens",
			"millau-to-rialto",
			"--source-host",
			"127.0.0.1",
			"--source-port",
			"9000",
			"--source-signer",
			"//Alice",
			"--source-balance",
			"8000000000",
			"--target-host",
			"127.0.0.1",
			"--target-port",
			"9001",
			"--target-signer",
			"//Bob",
			"--target-balance",
			"9000000000",
			"--target-to-source-conversion-rate-override",
			"metric",
			"--source-to-target-conversion-rate-override",
			"84.56",
			"lock-until-block",
			"--blocks-before-expire",
			"1",
		]);

		assert_eq!(
			swap_tokens,
			SwapTokens {
				bridge: SwapTokensBridge::MillauToRialto,
				source: SourceConnectionParams {
					source_host: "127.0.0.1".into(),
					source_port: 9000,
					source_secure: false,
					source_runtime_version: SourceRuntimeVersionParams {
						source_version_mode: RuntimeVersionType::Bundle,
						source_spec_version: None,
						source_transaction_version: None,
					}
				},
				source_sign: SourceSigningParams {
					source_signer: Some("//Alice".into()),
					source_signer_password: None,
					source_signer_file: None,
					source_signer_password_file: None,
					source_transactions_mortality: None,
				},
				target: TargetConnectionParams {
					target_host: "127.0.0.1".into(),
					target_port: 9001,
					target_secure: false,
					target_runtime_version: TargetRuntimeVersionParams {
						target_version_mode: RuntimeVersionType::Bundle,
						target_spec_version: None,
						target_transaction_version: None,
					}
				},
				target_sign: TargetSigningParams {
					target_signer: Some("//Bob".into()),
					target_signer_password: None,
					target_signer_file: None,
					target_signer_password_file: None,
					target_transactions_mortality: None,
				},
				swap_type: TokenSwapType::LockUntilBlock {
					blocks_before_expire: 1,
					swap_nonce: None,
				},
				source_balance: Balance(8000000000),
				target_balance: Balance(9000000000),
				target_to_source_conversion_rate_override: Some(ConversionRateOverride::Metric),
				source_to_target_conversion_rate_override: Some(ConversionRateOverride::Explicit(
					84.56
				)),
			}
		);
	}
}
