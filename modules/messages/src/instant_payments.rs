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

//! Implementation of `MessageDeliveryAndDispatchPayment` trait on top of `Currency` trait.
//!
//! The payment is first transferred to a special `relayers-fund` account and only transferred
//! to the actual relayer in case confirmation is received.

use crate::OutboundMessages;

use bp_messages::{
	source_chain::{MessageDeliveryAndDispatchPayment, RelayersRewards, SenderOrigin},
	LaneId, MessageKey, MessageNonce, UnrewardedRelayer,
};
use codec::Encode;
use frame_support::traits::{Currency as CurrencyT, ExistenceRequirement, Get};
use num_traits::{SaturatingAdd, Zero};
use sp_runtime::traits::Saturating;
use sp_std::{collections::vec_deque::VecDeque, fmt::Debug, ops::RangeInclusive};

/// Error that occurs when message fee is non-zero, but payer is not defined.
const NON_ZERO_MESSAGE_FEE_CANT_BE_PAID_BY_NONE: &str =
	"Non-zero message fee can't be paid by <None>";

/// Instant message payments made in given currency.
///
/// The balance is initially reserved in a special `relayers-fund` account, and transferred
/// to the relayer when message delivery is confirmed.
///
/// Additionally, confirmation transaction submitter (`confirmation_relayer`) is reimbursed
/// with the confirmation rewards (part of message fee, reserved to pay for delivery confirmation).
///
/// NOTE The `relayers-fund` account must always exist i.e. be over Existential Deposit (ED; the
/// pallet enforces that) to make sure that even if the message cost is below ED it is still paid
/// to the relayer account.
/// NOTE It's within relayer's interest to keep their balance above ED as well, to make sure they
/// can receive the payment.
pub struct InstantCurrencyPayments<T, I, Currency, GetConfirmationFee> {
	_phantom: sp_std::marker::PhantomData<(T, I, Currency, GetConfirmationFee)>,
}

impl<T, I, Currency, GetConfirmationFee>
	MessageDeliveryAndDispatchPayment<T::Origin, T::AccountId, Currency::Balance>
	for InstantCurrencyPayments<T, I, Currency, GetConfirmationFee>
where
	T: frame_system::Config + crate::Config<I>,
	I: 'static,
	T::Origin: SenderOrigin<T::AccountId>,
	Currency: CurrencyT<T::AccountId, Balance = T::OutboundMessageFee>,
	Currency::Balance: From<MessageNonce>,
	GetConfirmationFee: Get<Currency::Balance>,
{
	type Error = &'static str;

	fn pay_delivery_and_dispatch_fee(
		submitter: &T::Origin,
		fee: &Currency::Balance,
		relayer_fund_account: &T::AccountId,
	) -> Result<(), Self::Error> {
		let submitter_account = match submitter.linked_account() {
			Some(submitter_account) => submitter_account,
			None if !fee.is_zero() => {
				// if we'll accept some message that has declared that the `fee` has been paid but
				// it isn't actually paid, then it'll lead to problems with delivery confirmation
				// payments (see `pay_relayer_rewards` && `confirmation_relayer` in particular)
				return Err(NON_ZERO_MESSAGE_FEE_CANT_BE_PAID_BY_NONE)
			},
			None => {
				// message lane verifier has accepted the message before, so this message
				// is unpaid **by design**
				// => let's just do nothing
				return Ok(())
			},
		};

		if !frame_system::Pallet::<T>::account_exists(relayer_fund_account) {
			return Err("The relayer fund account must exist for the message lanes pallet to work correctly.");
		}

		Currency::transfer(
			&submitter_account,
			relayer_fund_account,
			*fee,
			// it's fine for the submitter to go below Existential Deposit and die.
			ExistenceRequirement::AllowDeath,
		)
		.map_err(Into::into)
	}

	fn pay_relayers_rewards(
		lane_id: LaneId,
		messages_relayers: VecDeque<UnrewardedRelayer<T::AccountId>>,
		confirmation_relayer: &T::AccountId,
		received_range: &RangeInclusive<MessageNonce>,
		relayer_fund_account: &T::AccountId,
	) {
		let relayers_rewards =
			cal_relayers_rewards::<T, I>(lane_id, messages_relayers, received_range);
		if !relayers_rewards.is_empty() {
			pay_relayers_rewards::<Currency, _>(
				confirmation_relayer,
				relayers_rewards,
				relayer_fund_account,
				GetConfirmationFee::get(),
			);
		}
	}
}

/// Calculate the relayers rewards
pub(crate) fn cal_relayers_rewards<T, I>(
	lane_id: LaneId,
	messages_relayers: VecDeque<UnrewardedRelayer<T::AccountId>>,
	received_range: &RangeInclusive<MessageNonce>,
) -> RelayersRewards<T::AccountId, T::OutboundMessageFee>
where
	T: frame_system::Config + crate::Config<I>,
	I: 'static,
{
	// remember to reward relayers that have delivered messages
	// this loop is bounded by `T::MaxUnrewardedRelayerEntriesAtInboundLane` on the bridged chain
	let mut relayers_rewards: RelayersRewards<_, T::OutboundMessageFee> = RelayersRewards::new();
	for entry in messages_relayers {
		let nonce_begin = sp_std::cmp::max(entry.messages.begin, *received_range.start());
		let nonce_end = sp_std::cmp::min(entry.messages.end, *received_range.end());

		// loop won't proceed if current entry is ahead of received range (begin > end).
		// this loop is bound by `T::MaxUnconfirmedMessagesAtInboundLane` on the bridged chain
		let mut relayer_reward = relayers_rewards.entry(entry.relayer).or_default();
		for nonce in nonce_begin..nonce_end + 1 {
			let message_data = OutboundMessages::<T, I>::get(MessageKey { lane_id, nonce })
				.expect("message was just confirmed; we never prune unconfirmed messages; qed");
			relayer_reward.reward = relayer_reward.reward.saturating_add(&message_data.fee);
			relayer_reward.messages += 1;
		}
	}
	relayers_rewards
}

/// Pay rewards to given relayers, optionally rewarding confirmation relayer.
fn pay_relayers_rewards<Currency, AccountId>(
	confirmation_relayer: &AccountId,
	relayers_rewards: RelayersRewards<AccountId, Currency::Balance>,
	relayer_fund_account: &AccountId,
	confirmation_fee: Currency::Balance,
) where
	AccountId: Debug + Encode + PartialEq,
	Currency: CurrencyT<AccountId>,
	Currency::Balance: From<u64>,
{
	// reward every relayer except `confirmation_relayer`
	let mut confirmation_relayer_reward = Currency::Balance::zero();
	for (relayer, reward) in relayers_rewards {
		let mut relayer_reward = reward.reward;

		if relayer != *confirmation_relayer {
			// If delivery confirmation is submitted by other relayer, let's deduct confirmation fee
			// from relayer reward.
			//
			// If confirmation fee has been increased (or if it was the only component of message
			// fee), then messages relayer may receive zero reward.
			let mut confirmation_reward = confirmation_fee.saturating_mul(reward.messages.into());
			if confirmation_reward > relayer_reward {
				confirmation_reward = relayer_reward;
			}
			relayer_reward = relayer_reward.saturating_sub(confirmation_reward);
			confirmation_relayer_reward =
				confirmation_relayer_reward.saturating_add(confirmation_reward);
		} else {
			// If delivery confirmation is submitted by this relayer, let's add confirmation fee
			// from other relayers to this relayer reward.
			confirmation_relayer_reward = confirmation_relayer_reward.saturating_add(reward.reward);
			continue
		}

		pay_relayer_reward::<Currency, _>(relayer_fund_account, &relayer, relayer_reward);
	}

	// finally - pay reward to confirmation relayer
	pay_relayer_reward::<Currency, _>(
		relayer_fund_account,
		confirmation_relayer,
		confirmation_relayer_reward,
	);
}

/// Transfer funds from relayers fund account to given relayer.
fn pay_relayer_reward<Currency, AccountId>(
	relayer_fund_account: &AccountId,
	relayer_account: &AccountId,
	reward: Currency::Balance,
) where
	AccountId: Debug,
	Currency: CurrencyT<AccountId>,
{
	if reward.is_zero() {
		return
	}

	let pay_result = Currency::transfer(
		relayer_fund_account,
		relayer_account,
		reward,
		// the relayer fund account must stay above ED (needs to be pre-funded)
		ExistenceRequirement::KeepAlive,
	);

	match pay_result {
		Ok(_) => log::trace!(
			target: "runtime::bridge-messages",
			"Rewarded relayer {:?} with {:?}",
			relayer_account,
			reward,
		),
		Err(error) => log::trace!(
			target: "runtime::bridge-messages",
			"Failed to pay relayer {:?} reward {:?}: {:?}",
			relayer_account,
			reward,
			error,
		),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::mock::{
		run_test, AccountId as TestAccountId, Balance as TestBalance, Origin, TestRuntime,
	};
	use bp_messages::source_chain::RelayerRewards;

	type Balances = pallet_balances::Pallet<TestRuntime>;

	const RELAYER_1: TestAccountId = 1;
	const RELAYER_2: TestAccountId = 2;
	const RELAYER_3: TestAccountId = 3;
	const RELAYERS_FUND_ACCOUNT: TestAccountId = crate::mock::ENDOWED_ACCOUNT;

	fn relayers_rewards() -> RelayersRewards<TestAccountId, TestBalance> {
		vec![
			(RELAYER_1, RelayerRewards { reward: 100, messages: 2 }),
			(RELAYER_2, RelayerRewards { reward: 100, messages: 3 }),
		]
		.into_iter()
		.collect()
	}

	#[test]
	fn pay_delivery_and_dispatch_fee_fails_on_non_zero_fee_and_unknown_payer() {
		frame_support::parameter_types! {
			const GetConfirmationFee: TestBalance = 0;
		};

		run_test(|| {
			let result = InstantCurrencyPayments::<
				TestRuntime,
				(),
				Balances,
				GetConfirmationFee,
			>::pay_delivery_and_dispatch_fee(
				&Origin::root(),
				&100,
				&RELAYERS_FUND_ACCOUNT,
			);
			assert_eq!(result, Err(NON_ZERO_MESSAGE_FEE_CANT_BE_PAID_BY_NONE));
		});
	}

	#[test]
	fn pay_delivery_and_dispatch_succeeds_on_zero_fee_and_unknown_payer() {
		frame_support::parameter_types! {
			const GetConfirmationFee: TestBalance = 0;
		};

		run_test(|| {
			let result = InstantCurrencyPayments::<
				TestRuntime,
				(),
				Balances,
				GetConfirmationFee,
			>::pay_delivery_and_dispatch_fee(
				&Origin::root(),
				&0,
				&RELAYERS_FUND_ACCOUNT,
			);
			assert!(result.is_ok());
		});
	}

	#[test]
	fn confirmation_relayer_is_rewarded_if_it_has_also_delivered_messages() {
		run_test(|| {
			pay_relayers_rewards::<Balances, _>(
				&RELAYER_2,
				relayers_rewards(),
				&RELAYERS_FUND_ACCOUNT,
				10,
			);

			assert_eq!(Balances::free_balance(&RELAYER_1), 80);
			assert_eq!(Balances::free_balance(&RELAYER_2), 120);
		});
	}

	#[test]
	fn confirmation_relayer_is_rewarded_if_it_has_not_delivered_any_delivered_messages() {
		run_test(|| {
			pay_relayers_rewards::<Balances, _>(
				&RELAYER_3,
				relayers_rewards(),
				&RELAYERS_FUND_ACCOUNT,
				10,
			);

			assert_eq!(Balances::free_balance(&RELAYER_1), 80);
			assert_eq!(Balances::free_balance(&RELAYER_2), 70);
			assert_eq!(Balances::free_balance(&RELAYER_3), 50);
		});
	}

	#[test]
	fn only_confirmation_relayer_is_rewarded_if_confirmation_fee_has_significantly_increased() {
		run_test(|| {
			pay_relayers_rewards::<Balances, _>(
				&RELAYER_3,
				relayers_rewards(),
				&RELAYERS_FUND_ACCOUNT,
				1000,
			);

			assert_eq!(Balances::free_balance(&RELAYER_1), 0);
			assert_eq!(Balances::free_balance(&RELAYER_2), 0);
			assert_eq!(Balances::free_balance(&RELAYER_3), 200);
		});
	}
}
