// Copyright (C) Parity Technologies (UK) Ltd.
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

//! # XCM-Builder
//!
//! Types and helpers for *building* XCM configuration.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(test)]
mod tests;

#[cfg(feature = "std")]
pub mod test_utils;

mod location_conversion;
#[allow(deprecated)]
pub use location_conversion::ForeignChainAliasAccount;
pub use location_conversion::{
	Account32Hash, AccountId32Aliases, AccountKey20Aliases, AliasesIntoAccountId32,
	ChildParachainConvertsVia, DescribeAccountId32Terminal, DescribeAccountIdTerminal,
	DescribeAccountKey20Terminal, DescribeAllTerminal, DescribeBodyTerminal, DescribeFamily,
	DescribeLocation, DescribePalletTerminal, DescribeTerminus, GlobalConsensusConvertsFor,
	GlobalConsensusParachainConvertsFor, HashedDescription, ParentIsPreset,
	SiblingParachainConvertsVia,
};

mod origin_conversion;
pub use origin_conversion::{
	BackingToPlurality, ChildParachainAsNative, ChildSystemParachainAsSuperuser, EnsureXcmOrigin,
	OriginToPluralityVoice, ParentAsSuperuser, RelayChainAsNative, SiblingParachainAsNative,
	SiblingSystemParachainAsSuperuser, SignedAccountId32AsNative, SignedAccountKey20AsNative,
	SignedToAccountId32, SovereignSignedViaLocation,
};

mod asset_conversion;
pub use asset_conversion::{
	AsPrefixedGeneralIndex, ConvertedAbstractId, ConvertedConcreteId, MatchedConvertedConcreteId,
};
#[allow(deprecated)]
pub use asset_conversion::{ConvertedAbstractAssetId, ConvertedConcreteAssetId};

mod barriers;
pub use barriers::{
	AllowExplicitUnpaidExecutionFrom, AllowKnownQueryResponses, AllowSubscriptionsFrom,
	AllowTopLevelPaidExecutionFrom, AllowUnpaidExecutionFrom, DenyReserveTransferToRelayChain,
	DenyThenTry, IsChildSystemParachain, RespectSuspension, TakeWeightCredit, TrailingSetTopicAsId,
	WithComputedOrigin,
};

mod process_xcm_message;
pub use process_xcm_message::ProcessXcmMessage;

mod currency_adapter;
pub use currency_adapter::CurrencyAdapter;

mod fungibles_adapter;
pub use fungibles_adapter::{
	AssetChecking, DualMint, FungiblesAdapter, FungiblesMutateAdapter, FungiblesTransferAdapter,
	LocalMint, MintLocation, NoChecking, NonLocalMint,
};

mod nonfungibles_adapter;
pub use nonfungibles_adapter::{
	NonFungiblesAdapter, NonFungiblesMutateAdapter, NonFungiblesTransferAdapter,
};

mod weight;
pub use weight::{
	FixedRateOfFungible, FixedWeightBounds, TakeRevenue, UsingComponents, WeightInfoBounds,
};

mod matches_token;
pub use matches_token::{IsAbstract, IsConcrete};

mod matcher;
pub use matcher::{CreateMatcher, MatchXcm, Matcher};

mod filter_asset_location;
pub use filter_asset_location::{Case, NativeAsset};

mod routing;
pub use routing::{WithTopicSource, WithUniqueTopic};

mod universal_exports;
pub use universal_exports::{
	ensure_is_remote, BridgeBlobDispatcher, BridgeMessage, DispatchBlob, DispatchBlobError,
	ExporterFor, HaulBlob, HaulBlobError, HaulBlobExporter, NetworkExportTable,
	SovereignPaidRemoteExporter, UnpaidLocalExporter, UnpaidRemoteExporter,
};

mod origin_aliases;
pub use origin_aliases::AliasForeignAccountId32;

mod pay;
pub use pay::{FixedLocation, LocatableAssetId, PayAccountId32OnChainOverXcm, PayOverXcm};
