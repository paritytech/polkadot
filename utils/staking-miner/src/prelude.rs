/// Types that we don't fetch from a particular runtime and just assume that they are constant all
/// of the place.
///
/// It is actually easy to convert the rest as well, but it'll be a lot of noise in our codebase,
/// needing to sprinkle `any_runtime` in a few extra places.

/// The account id type.
pub type AccountId = core_primitives::AccountId;
/// The block number type.
pub type BlockNumber = core_primitives::BlockNumber;
/// The balance type.
pub type Balance = core_primitives::Balance;
/// The signature type.
pub type Signature = core_primitives::Signature;
/// The index of an account.
pub type Index = core_primitives::AccountIndex;

pub const DEFAULT_URI: &'static str = "wss://rpc.polkadot.io";
pub const LOG_TARGET: &'static str = "staking-miner";

pub use pallet_election_provider_multi_phase as EPM;
pub use sp_runtime::traits::{Block as BlockT, Header as HeaderT};

// TODO: remove this one.
pub type EPMPalletOf<T> = EPM::Pallet<T>;
pub type StakingPalletOf<T> = pallet_staking::Pallet<T>;
pub type Ext = sp_io::TestExternalities;

// TODO: we 'strongly' assume this type of crypto;
pub type Pair = sp_core::sr25519::Pair;
