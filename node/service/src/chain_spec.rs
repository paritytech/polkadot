// Copyright 2017-2020 Parity Technologies (UK) Ltd.
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

//! Polkadot chain configurations.

use beefy_primitives::crypto::AuthorityId as BeefyId;
use grandpa::AuthorityId as GrandpaId;
#[cfg(feature = "kusama-native")]
use kusama_runtime as kusama;
#[cfg(feature = "kusama-native")]
use kusama_runtime_constants::currency::UNITS as KSM;
use pallet_im_online::sr25519::AuthorityId as ImOnlineId;
use pallet_staking::Forcing;
use polkadot_primitives::v2::{AccountId, AccountPublic, AssignmentId, ValidatorId};
#[cfg(feature = "polkadot-native")]
use polkadot_runtime as polkadot;
#[cfg(feature = "polkadot-native")]
use polkadot_runtime_constants::currency::UNITS as DOT;
use sp_authority_discovery::AuthorityId as AuthorityDiscoveryId;
use sp_consensus_babe::AuthorityId as BabeId;

#[cfg(feature = "rococo-native")]
use rococo_runtime as rococo;
#[cfg(feature = "rococo-native")]
use rococo_runtime_constants::currency::UNITS as ROC;
use sc_chain_spec::{ChainSpecExtension, ChainType};
use serde::{Deserialize, Serialize};
use sp_core::{sr25519, Pair, Public};
use sp_runtime::{traits::IdentifyAccount, Perbill};
use telemetry::TelemetryEndpoints;
#[cfg(feature = "westend-native")]
use westend_runtime as westend;
#[cfg(feature = "westend-native")]
use westend_runtime_constants::currency::UNITS as WND;

#[cfg(feature = "polkadot-native")]
const POLKADOT_STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
#[cfg(feature = "kusama-native")]
const KUSAMA_STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
#[cfg(feature = "westend-native")]
const WESTEND_STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
#[cfg(feature = "rococo-native")]
const ROCOCO_STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
#[cfg(feature = "rococo-native")]
const VERSI_STAGING_TELEMETRY_URL: &str = "wss://telemetry.polkadot.io/submit/";
const DEFAULT_PROTOCOL_ID: &str = "dot";

/// Node `ChainSpec` extensions.
///
/// Additional parameters for some Substrate core modules,
/// customizable from the chain spec.
#[derive(Default, Clone, Serialize, Deserialize, ChainSpecExtension)]
#[serde(rename_all = "camelCase")]
pub struct Extensions {
	/// Block numbers with known hashes.
	pub fork_blocks: sc_client_api::ForkBlocks<polkadot_primitives::v2::Block>,
	/// Known bad block hashes.
	pub bad_blocks: sc_client_api::BadBlocks<polkadot_primitives::v2::Block>,
	/// The light sync state.
	///
	/// This value will be set by the `sync-state rpc` implementation.
	pub light_sync_state: sc_sync_state_rpc::LightSyncStateExtension,
}

/// The `ChainSpec` parameterized for the polkadot runtime.
#[cfg(feature = "polkadot-native")]
pub type PolkadotChainSpec = service::GenericChainSpec<polkadot::GenesisConfig, Extensions>;

// Dummy chain spec, in case when we don't have the native runtime.
pub type DummyChainSpec = service::GenericChainSpec<(), Extensions>;

// Dummy chain spec, but that is fine when we don't have the native runtime.
#[cfg(not(feature = "polkadot-native"))]
pub type PolkadotChainSpec = DummyChainSpec;

/// The `ChainSpec` parameterized for the kusama runtime.
#[cfg(feature = "kusama-native")]
pub type KusamaChainSpec = service::GenericChainSpec<kusama::GenesisConfig, Extensions>;

/// The `ChainSpec` parameterized for the kusama runtime.
// Dummy chain spec, but that is fine when we don't have the native runtime.
#[cfg(not(feature = "kusama-native"))]
pub type KusamaChainSpec = DummyChainSpec;

/// The `ChainSpec` parameterized for the westend runtime.
#[cfg(feature = "westend-native")]
pub type WestendChainSpec = service::GenericChainSpec<westend::GenesisConfig, Extensions>;

/// The `ChainSpec` parameterized for the westend runtime.
// Dummy chain spec, but that is fine when we don't have the native runtime.
#[cfg(not(feature = "westend-native"))]
pub type WestendChainSpec = DummyChainSpec;

/// The `ChainSpec` parameterized for the rococo runtime.
#[cfg(feature = "rococo-native")]
pub type RococoChainSpec = service::GenericChainSpec<RococoGenesisExt, Extensions>;

/// The `ChainSpec` parameterized for the `versi` runtime.
///
/// As of now `Versi` will just be a clone of `Rococo`, until we need it to differ.
pub type VersiChainSpec = RococoChainSpec;

/// The `ChainSpec` parameterized for the rococo runtime.
// Dummy chain spec, but that is fine when we don't have the native runtime.
#[cfg(not(feature = "rococo-native"))]
pub type RococoChainSpec = DummyChainSpec;

/// Extension for the Rococo genesis config to support a custom changes to the genesis state.
#[derive(serde::Serialize, serde::Deserialize)]
#[cfg(feature = "rococo-native")]
pub struct RococoGenesisExt {
	/// The runtime genesis config.
	runtime_genesis_config: rococo::GenesisConfig,
	/// The session length in blocks.
	///
	/// If `None` is supplied, the default value is used.
	session_length_in_blocks: Option<u32>,
}

#[cfg(feature = "rococo-native")]
impl sp_runtime::BuildStorage for RococoGenesisExt {
	fn assimilate_storage(&self, storage: &mut sp_core::storage::Storage) -> Result<(), String> {
		sp_state_machine::BasicExternalities::execute_with_storage(storage, || {
			if let Some(length) = self.session_length_in_blocks.as_ref() {
				rococo_runtime_constants::time::EpochDurationInBlocks::set(length);
			}
		});
		self.runtime_genesis_config.assimilate_storage(storage)
	}
}

pub fn polkadot_config() -> Result<PolkadotChainSpec, String> {
	PolkadotChainSpec::from_json_bytes(&include_bytes!("../chain-specs/polkadot.json")[..])
}

pub fn kusama_config() -> Result<KusamaChainSpec, String> {
	KusamaChainSpec::from_json_bytes(&include_bytes!("../chain-specs/kusama.json")[..])
}

pub fn westend_config() -> Result<WestendChainSpec, String> {
	WestendChainSpec::from_json_bytes(&include_bytes!("../chain-specs/westend.json")[..])
}

pub fn rococo_config() -> Result<RococoChainSpec, String> {
	RococoChainSpec::from_json_bytes(&include_bytes!("../chain-specs/rococo.json")[..])
}

/// This is a temporary testnet that uses the same runtime as rococo.
pub fn wococo_config() -> Result<RococoChainSpec, String> {
	RococoChainSpec::from_json_bytes(&include_bytes!("../chain-specs/wococo.json")[..])
}

/// The default parachains host configuration.
#[cfg(any(
	feature = "rococo-native",
	feature = "kusama-native",
	feature = "westend-native",
	feature = "polkadot-native"
))]
fn default_parachains_host_configuration(
) -> polkadot_runtime_parachains::configuration::HostConfiguration<
	polkadot_primitives::v2::BlockNumber,
> {
	use polkadot_primitives::v2::{MAX_CODE_SIZE, MAX_POV_SIZE};

	polkadot_runtime_parachains::configuration::HostConfiguration {
		validation_upgrade_cooldown: 2u32,
		validation_upgrade_delay: 2,
		code_retention_period: 1200,
		max_code_size: MAX_CODE_SIZE,
		max_pov_size: MAX_POV_SIZE,
		max_head_data_size: 32 * 1024,
		group_rotation_frequency: 20,
		chain_availability_period: 4,
		thread_availability_period: 4,
		max_upward_queue_count: 8,
		max_upward_queue_size: 1024 * 1024,
		max_downward_message_size: 1024 * 1024,
		ump_service_total_weight: 100_000_000_000,
		max_upward_message_size: 50 * 1024,
		max_upward_message_num_per_candidate: 5,
		hrmp_sender_deposit: 0,
		hrmp_recipient_deposit: 0,
		hrmp_channel_max_capacity: 8,
		hrmp_channel_max_total_size: 8 * 1024,
		hrmp_max_parachain_inbound_channels: 4,
		hrmp_max_parathread_inbound_channels: 4,
		hrmp_channel_max_message_size: 1024 * 1024,
		hrmp_max_parachain_outbound_channels: 4,
		hrmp_max_parathread_outbound_channels: 4,
		hrmp_max_message_num_per_candidate: 5,
		dispute_period: 6,
		no_show_slots: 2,
		n_delay_tranches: 25,
		needed_approvals: 2,
		relay_vrf_modulo_samples: 2,
		zeroth_delay_tranche_width: 0,
		minimum_validation_upgrade_delay: 5,
		..Default::default()
	}
}

#[cfg(any(
	feature = "rococo-native",
	feature = "kusama-native",
	feature = "westend-native",
	feature = "polkadot-native"
))]
#[test]
fn default_parachains_host_configuration_is_consistent() {
	default_parachains_host_configuration().panic_if_not_consistent();
}

#[cfg(feature = "polkadot-native")]
fn polkadot_session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	im_online: ImOnlineId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
) -> polkadot::SessionKeys {
	polkadot::SessionKeys {
		babe,
		grandpa,
		im_online,
		para_validator,
		para_assignment,
		authority_discovery,
	}
}

#[cfg(feature = "kusama-native")]
fn kusama_session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	im_online: ImOnlineId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
) -> kusama::SessionKeys {
	kusama::SessionKeys {
		babe,
		grandpa,
		im_online,
		para_validator,
		para_assignment,
		authority_discovery,
	}
}

#[cfg(feature = "westend-native")]
fn westend_session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	im_online: ImOnlineId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
) -> westend::SessionKeys {
	westend::SessionKeys {
		babe,
		grandpa,
		im_online,
		para_validator,
		para_assignment,
		authority_discovery,
	}
}

#[cfg(feature = "rococo-native")]
fn rococo_session_keys(
	babe: BabeId,
	grandpa: GrandpaId,
	im_online: ImOnlineId,
	para_validator: ValidatorId,
	para_assignment: AssignmentId,
	authority_discovery: AuthorityDiscoveryId,
	beefy: BeefyId,
) -> rococo_runtime::SessionKeys {
	rococo_runtime::SessionKeys {
		babe,
		grandpa,
		im_online,
		para_validator,
		para_assignment,
		authority_discovery,
		beefy,
	}
}

#[cfg(feature = "polkadot-native")]
fn polkadot_staging_testnet_config_genesis(wasm_binary: &[u8]) -> polkadot::GenesisConfig {
	// subkey inspect "$SECRET"
	let endowed_accounts = vec![];

	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)> = vec![];

	const ENDOWMENT: u128 = 1_000_000 * DOT;
	const STASH: u128 = 100 * DOT;

	polkadot::GenesisConfig {
		system: polkadot::SystemConfig { code: wasm_binary.to_vec() },
		balances: polkadot::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		},
		indices: polkadot::IndicesConfig { indices: vec![] },
		session: polkadot::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						polkadot_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: polkadot::StakingConfig {
			validator_count: 50,
			minimum_validator_count: 4,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, polkadot::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::ForceNone,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		phragmen_election: Default::default(),
		democracy: Default::default(),
		council: polkadot::CouncilConfig { members: vec![], phantom: Default::default() },
		technical_committee: polkadot::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		},
		technical_membership: Default::default(),
		babe: polkadot::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(polkadot::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: polkadot::AuthorityDiscoveryConfig { keys: vec![] },
		claims: polkadot::ClaimsConfig { claims: vec![], vesting: vec![] },
		vesting: polkadot::VestingConfig { vesting: vec![] },
		treasury: Default::default(),
		hrmp: Default::default(),
		configuration: polkadot::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		paras: Default::default(),
		xcm_pallet: Default::default(),
	}
}

#[cfg(feature = "westend-native")]
fn westend_staging_testnet_config_genesis(wasm_binary: &[u8]) -> westend::GenesisConfig {
	use hex_literal::hex;
	use sp_core::crypto::UncheckedInto;

	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		// 5DaVh5WRfazkGaKhx1jUu6hjz7EmRe4dtW6PKeVLim84KLe8
		hex!["42f4a4b3e0a89c835ee696205caa90dd85c8ea1d7364b646328ee919a6b2fc1e"].into(),
	];
	// SECRET='...' ./scripts/prepare-test-net.sh 4
	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)> = vec![
		(
			//5ERCqy118nnXDai8g4t3MjdX7ZC5PrQzQpe9vwex5cELWqbt
			hex!["681af4f93073484e1acd6b27395d0d258f1a6b158c808846c8fd05ee2435056e"].into(),
			//5GTS114cfQNBgpQULhMaNCPXGds6NokegCnikxDe1vqANhtn
			hex!["c2463372598ebabd21ee5bc33e1d7e77f391d2df29ce2fbe6bed0d13be629a45"].into(),
			//5FhGbceKeH7fuGogcBwd28ZCkAwDGYBADCTeHiYrvx2ztyRd
			hex!["a097bfc6a33499ed843b711f52f523f8a7174f798a9f98620e52f4170dbe2948"]
				.unchecked_into(),
			//5Es7nDkJt2by5qVCCD7PZJdp76KJw1LdRCiNst5S5f4eecnz
			hex!["7bde49dda82c2c9f082b807ef3ceebff96437d67b3e630c584db7a220ecafacf"]
				.unchecked_into(),
			//5D4e8zRjaYzFamqChGPPtu26PcKbKgUrhb7WqcNbKa2RDFUR
			hex!["2c2fb730a7d9138e6d62fcf516f9ecc2d712af3f2f03ca330c9564b8c0c1bb33"]
				.unchecked_into(),
			//5DD3JY5ENkjcgVFbVSgUbZv7WmrnyJ8bxxu56ee6hZFiRdnh
			hex!["3297a8622988cc23dd9c131e3fb8746d49e007f6e58a81d43420cd539e250e4c"]
				.unchecked_into(),
			//5Gpodowhud8FG9xENXR5YwTFbUAWyoEtw7sYFytFsG4z7SU6
			hex!["d2932edf775088bd088dc5a112ad867c24cc95858f77f8a1ab014de8d4f96a3f"]
				.unchecked_into(),
			//5GUMj8tnjL3PJZgXoiWtgLCaMVNHBNeSeTqDsvcxmaVAjKn9
			hex!["c2fb0f74591a00555a292bc4882d3158bafc4c632124cb60681f164ef81bcf72"]
				.unchecked_into(),
		),
		(
			//5HgDCznTkHKUjzPkQoTZGWbvbyqB7sqHDBPDKdF1FyVYM7Er
			hex!["f8418f189f84814fd40cc1b2e90873e72ea789487f3b98ed42811ba76d10fc37"].into(),
			//5GQTryeFwuvgmZ2tH5ZeAKZHRM9ch5WGVGo6ND9P8f9uMsNY
			hex!["c002bb4af4a1bd2f33d104aef8a41878fe1ac94ba007029c4dfdefa8b698d043"].into(),
			//5C7YkWSVH1zrpsE5KwW1ua1qatyphzYxiZrL24mjkxz7mUbn
			hex!["022b14fbcf65a93b81f453105b9892c3fc4aa74c22c53b4abab019e1d58fbd41"]
				.unchecked_into(),
			//5GwFC6Tmg4fhj4PxSqHycgJxi3PDfnC9RGDsNHoRwAvXvpnZ
			hex!["d77cafd3b32c8b52b0e2780a586a6e527c94f1bdec117c4e4acb0a491461ffa3"]
				.unchecked_into(),
			//5DSVrGURuDuh8Luzo8FYq7o2NWiUSLSN6QAVNrj9BtswWH6R
			hex!["3cdb36a5a14715999faffd06c5b9e5dcdc24d4b46bc3e4df1aaad266112a7b49"]
				.unchecked_into(),
			//5DLEG2AupawCXGwhJtrzBRc3zAhuP8V662dDrUTzAsCiB9Ec
			hex!["38134245c9919ecb20bf2eedbe943b69ba92ceb9eb5477b92b0afd3cb6ce2858"]
				.unchecked_into(),
			//5D83o9fDgnHxaKPkSx59hk8zYzqcgzN2mrf7cp8fiVEi7V4E
			hex!["2ec917690dc1d676002e3504c530b2595490aa5a4603d9cc579b9485b8d0d854"]
				.unchecked_into(),
			//5DwBJquZgncRWXFxj2ydbF8LBUPPUbiq86sXWXgm8Z38m8L2
			hex!["52bae9b8dedb8058dda93ec6f57d7e5a517c4c9f002a4636fada70fed0acf376"]
				.unchecked_into(),
		),
		(
			//5DMHpkRpQV7NWJFfn2zQxCLiAKv7R12PWFRPHKKk5X3JkYfP
			hex!["38e280b35d08db46019a210a944e4b7177665232ab679df12d6a8bbb317a2276"].into(),
			//5FbJpSHmFDe5FN3DVGe1R345ZePL9nhcC9V2Cczxo7q8q6rN
			hex!["9c0bc0e2469924d718ae683737f818a47c46b0612376ecca06a2ac059fe1f870"].into(),
			//5E5Pm3Udzxy26KGkLE5pc8JPfQrvkYHiaXWtuEfmQsBSgep9
			hex!["58fecadc2df8182a27e999e7e1fd7c99f8ec18f2a81f9a0db38b3653613f3f4d"]
				.unchecked_into(),
			//5FxcystSLHtaWoy2HEgRNerj9PrUs452B6AvHVnQZm5ZQmqE
			hex!["ac4d0c5e8f8486de05135c10a707f58aa29126d5eb28fdaaba00f9a505f5249d"]
				.unchecked_into(),
			//5E7KqVXaVGuAqiqMigpuH8oXHLVh4tmijmpJABLYANpjMkem
			hex!["5a781385a0235fe8594dd101ec55ef9ba01883f8563a0cdd37b89e0303f6a578"]
				.unchecked_into(),
			//5H9AybjkpyZ79yN5nHuBqs6RKuZPgM7aAVVvTQsDFovgXb2A
			hex!["e09570f62a062450d4406b4eb43e7f775ff954e37606646cd590d1818189501f"]
				.unchecked_into(),
			//5Ccgs7VwJKBawMbwMENDmj2eFAxhFdGksVHdk8aTAf4w7xox
			hex!["1864832dae34df30846d5cc65973f58a2d01b337d094b1284ec3466ecc90251d"]
				.unchecked_into(),
			//5EsSaZZ7niJs7hmAtp4QeK19AcAuTp7WXB7N7gRipVooerq4
			hex!["7c1d92535e6d94e21cffea6633a855a7e3c9684cd2f209e5ddbdeaf5111e395b"]
				.unchecked_into(),
		),
		(
			//5Ea11qhmGRntQ7pyEkEydbwxvfrYwGMKW6rPERU4UiSBB6rd
			hex!["6ed057d2c833c45629de2f14b9f6ce6df1edbf9421b7a638e1fb4828c2bd2651"].into(),
			//5CZomCZwPB78BZMZsCiy7WSpkpHhdrN8QTSyjcK3FFEZHBor
			hex!["1631ff446b3534d031adfc37b7f7aed26d2a6b3938d10496aab3345c54707429"].into(),
			//5CSM6vppouFHzAVPkVFWN76DPRUG7B9qwJe892ccfSfJ8M5f
			hex!["108188c43a7521e1abe737b343341c2179a3a89626c7b017c09a5b10df6f1c42"]
				.unchecked_into(),
			//5GwkG4std9KcjYi3ThSC7QWfhqokmYVvWEqTU9h7iswjhLnr
			hex!["d7de8a43f7ee49fa3b3aaf32fb12617ec9ff7b246a46ab14e9c9d259261117fa"]
				.unchecked_into(),
			//5CoUk3wrCGJAWbiJEcsVjYhnd2JAHvR59jBRbSw77YrBtRL1
			hex!["209f680bc501f9b59358efe3636c51fd61238a8659bac146db909aea2595284b"]
				.unchecked_into(),
			//5EcSu96wprFM7G2HfJTjYu8kMParnYGznSUNTsoEKXywEsgG
			hex!["70adf80395b3f59e4cab5d9da66d5a286a0b6e138652a06f72542e46912df922"]
				.unchecked_into(),
			//5Ge3sjpD43Cuy7rNoJQmE9WctgCn6Faw89Pe7xPs3i55eHwJ
			hex!["ca5f6b970b373b303f64801a0c2cadc4fc05272c6047a2560a27d0c65589ca1d"]
				.unchecked_into(),
			//5EFcjHLvB2z5vd5g63n4gABmhzP5iPsKvTwd8sjfvTehNNrk
			hex!["60cae7fa5a079d9fc8061d715fbcc35ef57c3b00005694c2badce22dcc5a9f1b"]
				.unchecked_into(),
		),
	];

	const ENDOWMENT: u128 = 1_000_000 * WND;
	const STASH: u128 = 100 * WND;

	westend::GenesisConfig {
		system: westend::SystemConfig { code: wasm_binary.to_vec() },
		balances: westend::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		},
		indices: westend::IndicesConfig { indices: vec![] },
		session: westend::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						westend_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: westend::StakingConfig {
			validator_count: 50,
			minimum_validator_count: 4,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, westend::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::ForceNone,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		babe: westend::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(westend::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: westend::AuthorityDiscoveryConfig { keys: vec![] },
		vesting: westend::VestingConfig { vesting: vec![] },
		sudo: westend::SudoConfig { key: Some(endowed_accounts[0].clone()) },
		hrmp: Default::default(),
		configuration: westend::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		paras: Default::default(),
		registrar: westend_runtime::RegistrarConfig {
			next_free_para_id: polkadot_primitives::v2::LOWEST_PUBLIC_ID,
		},
		xcm_pallet: Default::default(),
		nomination_pools: Default::default(),
	}
}

#[cfg(feature = "kusama-native")]
fn kusama_staging_testnet_config_genesis(wasm_binary: &[u8]) -> kusama::GenesisConfig {
	use hex_literal::hex;
	use sp_core::crypto::UncheckedInto;

	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		// 5CVFESwfkk7NmhQ6FwHCM9roBvr9BGa4vJHFYU8DnGQxrXvz
		hex!["12b782529c22032ed4694e0f6e7d486be7daa6d12088f6bc74d593b3900b8438"].into(),
	];

	// for i in 1 2 3 4; do for j in stash controller; do subkey inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in babe; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in grandpa; do subkey --ed25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in im_online; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	// for i in 1 2 3 4; do for j in para_validator para_assignment; do subkey --sr25519 inspect "$SECRET//$i//$j"; done; done
	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)> = vec![
		(
			// 5DD7Q4VEfPTLEdn11CnThoHT5f9xKCrnofWJL5SsvpTghaAT
			hex!["32a5718e87d16071756d4b1370c411bbbb947eb62f0e6e0b937d5cbfc0ea633b"].into(),
			// 5GNzaEqhrZAtUQhbMe2gn9jBuNWfamWFZHULryFwBUXyd1cG
			hex!["bee39fe862c85c91aaf343e130d30b643c6ea0b4406a980206f1df8331f7093b"].into(),
			// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
			hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"]
				.unchecked_into(),
			// 5EjvdwATjyFFikdZibVvx1q5uBHhphS2Mnsq5c7yfaYK25vm
			hex!["76620f7c98bce8619979c2b58cf2b0aff71824126d2b039358729dad993223db"]
				.unchecked_into(),
			// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
			hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"]
				.unchecked_into(),
			// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
			hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"]
				.unchecked_into(),
			// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
			hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"]
				.unchecked_into(),
			// 5FpewyS2VY8Cj3tKgSckq8ECkjd1HKHvBRnWhiHqRQsWfFC1
			hex!["a639b507ee1585e0b6498ff141d6153960794523226866d1b44eba3f25f36356"]
				.unchecked_into(),
		),
		(
			// 5G9VGb8ESBeS8Ca4or43RfhShzk9y7T5iTmxHk5RJsjZwsRx
			hex!["b496c98a405ceab59b9e970e59ef61acd7765a19b704e02ab06c1cdfe171e40f"].into(),
			// 5F7V9Y5FcxKXe1aroqvPeRiUmmeQwTFcL3u9rrPXcMuMiCNx
			hex!["86d3a7571dd60139d297e55d8238d0c977b2e208c5af088f7f0136b565b0c103"].into(),
			// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
			hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"]
				.unchecked_into(),
			// 5HBDAaybNqjmY7ww8ZcZZY1L5LHxvpnyfqJwoB7HhR6raTmG
			hex!["e2234d661bee4a04c38392c75d1566200aa9e6ae44dd98ee8765e4cc9af63cb7"]
				.unchecked_into(),
			// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
			hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"]
				.unchecked_into(),
			// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
			hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"]
				.unchecked_into(),
			// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
			hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"]
				.unchecked_into(),
			// 5GvuM53k1Z4nAB5zXJFgkRSHv4Bqo4BsvgbQWNWkiWZTMwWY
			hex!["765e46067adac4d1fe6c783aa2070dfa64a19f84376659e12705d1734b3eae01"]
				.unchecked_into(),
		),
		(
			// 5FzwpgGvk2kk9agow6KsywLYcPzjYc8suKej2bne5G5b9YU3
			hex!["ae12f70078a22882bf5135d134468f77301927aa67c376e8c55b7ff127ace115"].into(),
			// 5EqoZhVC2BcsM4WjvZNidu2muKAbu5THQTBKe3EjvxXkdP7A
			hex!["7addb914ec8486bbc60643d2647685dcc06373401fa80e09813b630c5831d54b"].into(),
			// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
			hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"]
				.unchecked_into(),
			// 5E8ULLQrDAtWhfnVfZmX41Yux86zNAwVJYguWJZVWrJvdhBe
			hex!["5b57ed1443c8967f461db1f6eb2ada24794d163a668f1cf9d9ce3235dfad8799"]
				.unchecked_into(),
			// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
			hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"]
				.unchecked_into(),
			// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
			hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"]
				.unchecked_into(),
			// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
			hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"]
				.unchecked_into(),
			// 5CXNq1mSKJT4Sc2CbyBBdANeSkbUvdWvE4czJjKXfBHi9sX5
			hex!["664eae1ca4713dd6abf8c15e6c041820cda3c60df97dc476c2cbf7cb82cb2d2e"]
				.unchecked_into(),
		),
		(
			// 5CFj6Kg9rmVn1vrqpyjau2ztyBzKeVdRKwNPiA3tqhB5HPqq
			hex!["0867dbb49721126df589db100dda728dc3b475cbf414dad8f72a1d5e84897252"].into(),
			// 5CwQXP6nvWzigFqNhh2jvCaW9zWVzkdveCJY3tz2MhXMjTon
			hex!["26ab2b4b2eba2263b1e55ceb48f687bb0018130a88df0712fbdaf6a347d50e2a"].into(),
			// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
			hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"]
				.unchecked_into(),
			// 5HGLmrZsiTFTPp3QoS1W8w9NxByt8PVq79reqvdxNcQkByqK
			hex!["e60d23f49e93c1c1f2d7c115957df5bbd7faf5ebf138d1e9d02e8b39a1f63df0"]
				.unchecked_into(),
			// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
			hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"]
				.unchecked_into(),
			// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
			hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"]
				.unchecked_into(),
			// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
			hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"]
				.unchecked_into(),
			// 5FCd9Y7RLNyxz5wnCAErfsLbXGG34L2BaZRHzhiJcMUMd5zd
			hex!["2adb17a5cafbddc7c3e00ec45b6951a8b12ce2264235b4def342513a767e5d3d"]
				.unchecked_into(),
		),
	];

	const ENDOWMENT: u128 = 1_000_000 * KSM;
	const STASH: u128 = 100 * KSM;

	kusama::GenesisConfig {
		system: kusama::SystemConfig { code: wasm_binary.to_vec() },
		balances: kusama::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		},
		indices: kusama::IndicesConfig { indices: vec![] },
		session: kusama::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						kusama_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: kusama::StakingConfig {
			validator_count: 50,
			minimum_validator_count: 4,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, kusama::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::ForceNone,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		phragmen_election: Default::default(),
		democracy: Default::default(),
		council: kusama::CouncilConfig { members: vec![], phantom: Default::default() },
		technical_committee: kusama::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		},
		technical_membership: Default::default(),
		babe: kusama::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(kusama::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: kusama::AuthorityDiscoveryConfig { keys: vec![] },
		claims: kusama::ClaimsConfig { claims: vec![], vesting: vec![] },
		vesting: kusama::VestingConfig { vesting: vec![] },
		treasury: Default::default(),
		hrmp: Default::default(),
		configuration: kusama::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		gilt: Default::default(),
		paras: Default::default(),
		xcm_pallet: Default::default(),
		nomination_pools: Default::default(),
	}
}

#[cfg(feature = "rococo-native")]
fn rococo_staging_testnet_config_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	use hex_literal::hex;
	use sp_core::crypto::UncheckedInto;

	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		// 5DwBmEFPXRESyEam5SsQF1zbWSCn2kCjyLW51hJHXe9vW4xs
		hex!["52bc71c1eca5353749542dfdf0af97bf764f9c2f44e860cd485f1cd86400f649"].into(),
	];

	// ./scripts/prepare-test-net.sh 8
	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
		BeefyId,
	)> = vec![
		(
			//5EHZkbp22djdbuMFH9qt1DVzSCvqi3zWpj6DAYfANa828oei
			hex!["62475fe5406a7cb6a64c51d0af9d3ab5c2151bcae982fb812f7a76b706914d6a"].into(),
			//5FeSEpi9UYYaWwXXb3tV88qtZkmSdB3mvgj3pXkxKyYLGhcd
			hex!["9e6e781a76810fe93187af44c79272c290c2b9e2b8b92ee11466cd79d8023f50"].into(),
			//5Fh6rDpMDhM363o1Z3Y9twtaCPfizGQWCi55BSykTQjGbP7H
			hex!["a076ef1280d768051f21d060623da3ab5b56944d681d303ed2d4bf658c5bed35"]
				.unchecked_into(),
			//5CPd3zoV9Aaah4xWucuDivMHJ2nEEmpdi864nPTiyRZp4t87
			hex!["0e6d7d1afbcc6547b92995a394ba0daed07a2420be08220a5a1336c6731f0bfa"]
				.unchecked_into(),
			//5F7BEa1LGFksUihyatf3dCDYneB8pWzVyavnByCsm5nBgezi
			hex!["86975a37211f8704e947a365b720f7a3e2757988eaa7d0f197e83dba355ef743"]
				.unchecked_into(),
			//5CP6oGfwqbEfML8efqm1tCZsUgRsJztp9L8ZkEUxA16W8PPz
			hex!["0e07a51d3213842f8e9363ce8e444255990a225f87e80a3d651db7841e1a0205"]
				.unchecked_into(),
			//5HQdwiDh8Qtd5dSNWajNYpwDvoyNWWA16Y43aEkCNactFc2b
			hex!["ec60e71fe4a567ef9fef99d4bbf37ffae70564b41aa6f94ef0317c13e0a5477b"]
				.unchecked_into(),
			//5HbSgM72xVuscsopsdeG3sCSCYdAeM1Tay9p79N6ky6vwDGq
			hex!["f49eae66a0ac9f610316906ec8f1a0928e20d7059d76a5ca53cbcb5a9b50dd3c"]
				.unchecked_into(),
			//5DPSWdgw38Spu315r6LSvYCggeeieBAJtP5A1qzuzKhqmjVu
			hex!["034f68c5661a41930c82f26a662276bf89f33467e1c850f2fb8ef687fe43d62276"]
				.unchecked_into(),
		),
		(
			//5DvH8oEjQPYhzCoQVo7WDU91qmQfLZvxe9wJcrojmJKebCmG
			hex!["520b48452969f6ddf263b664de0adb0c729d0e0ad3b0e5f3cb636c541bc9022a"].into(),
			//5ENZvCRzyXJJYup8bM6yEzb2kQHEb1NDpY2ZEyVGBkCfRdj3
			hex!["6618289af7ae8621981ffab34591e7a6486e12745dfa3fd3b0f7e6a3994c7b5b"].into(),
			//5DLjSUfqZVNAADbwYLgRvHvdzXypiV1DAEaDMjcESKTcqMoM
			hex!["38757d0de00a0c739e7d7984ef4bc01161bd61e198b7c01b618425c16bb5bd5f"]
				.unchecked_into(),
			//5HnDVBN9mD6mXyx8oryhDbJtezwNSj1VRXgLoYCBA6uEkiao
			hex!["fcd5f87a6fd5707a25122a01b4dac0a8482259df7d42a9a096606df1320df08d"]
				.unchecked_into(),
			//5DhyXZiuB1LvqYKFgT5tRpgGsN3is2cM9QxgW7FikvakbAZP
			hex!["48a910c0af90898f11bd57d37ceaea53c78994f8e1833a7ade483c9a84bde055"]
				.unchecked_into(),
			//5EPEWRecy2ApL5n18n3aHyU1956zXTRqaJpzDa9DoqiggNwF
			hex!["669a10892119453e9feb4e3f1ee8e028916cc3240022920ad643846fbdbee816"]
				.unchecked_into(),
			//5ES3fw5X4bndSgLNmtPfSbM2J1kLqApVB2CCLS4CBpM1UxUZ
			hex!["68bf52c482630a8d1511f2edd14f34127a7d7082219cccf7fd4c6ecdb535f80d"]
				.unchecked_into(),
			//5HeXbwb5PxtcRoopPZTp5CQun38atn2UudQ8p2AxR5BzoaXw
			hex!["f6f8fe475130d21165446a02fb1dbce3a7bf36412e5d98f4f0473aed9252f349"]
				.unchecked_into(),
			//5F7nTtN8MyJV4UsXpjg7tHSnfANXZ5KRPJmkASc1ZSH2Xoa5
			hex!["03a90c2bb6d3b7000020f6152fe2e5002fa970fd1f42aafb6c8edda8dacc2ea77e"]
				.unchecked_into(),
		),
		(
			//5FPMzsezo1PRxYbVpJMWK7HNbR2kUxidsAAxH4BosHa4wd6S
			hex!["92ef83665b39d7a565e11bf8d18d41d45a8011601c339e57a8ea88c8ff7bba6f"].into(),
			//5G6NQidFG7YiXsvV7hQTLGArir9tsYqD4JDxByhgxKvSKwRx
			hex!["b235f57244230589523271c27b8a490922ffd7dccc83b044feaf22273c1dc735"].into(),
			//5GpZhzAVg7SAtzLvaAC777pjquPEcNy1FbNUAG2nZvhmd6eY
			hex!["d2644c1ab2c63a3ad8d40ad70d4b260969e3abfe6d7e6665f50dc9f6365c9d2a"]
				.unchecked_into(),
			//5HAes2RQYPbYKbLBfKb88f4zoXv6pPA6Ke8CjN7dob3GpmSP
			hex!["e1b68fbd84333e31486c08e6153d9a1415b2e7e71b413702b7d64e9b631184a1"]
				.unchecked_into(),
			//5HTXBf36LXmkFWJLokNUK6fPxVpkr2ToUnB1pvaagdGu4c1T
			hex!["ee93e26259decb89afcf17ef2aa0fa2db2e1042fb8f56ecfb24d19eae8629878"]
				.unchecked_into(),
			//5FtAGDZYJKXkhVhAxCQrXmaP7EE2mGbBMfmKDHjfYDgq2BiU
			hex!["a8e61ffacafaf546283dc92d14d7cc70ea0151a5dd81fdf73ff5a2951f2b6037"]
				.unchecked_into(),
			//5CtK7JHv3h6UQZ44y54skxdwSVBRtuxwPE1FYm7UZVhg8rJV
			hex!["244f3421b310c68646e99cdbf4963e02067601f57756b072a4b19431448c186e"]
				.unchecked_into(),
			//5D4r6YaB6F7A7nvMRHNFNF6zrR9g39bqDJFenrcaFmTCRwfa
			hex!["2c57f81fd311c1ab53813c6817fe67f8947f8d39258252663b3384ab4195494d"]
				.unchecked_into(),
			//5EPoHj8uV4fFKQHYThc6Z9fDkU7B6ih2ncVzQuDdNFb8UyhF
			hex!["039d065fe4f9234f0a4f13cc3ae585f2691e9c25afa469618abb6645111f607a53"]
				.unchecked_into(),
		),
		(
			//5DMNx7RoX6d7JQ38NEM7DWRcW2THu92LBYZEWvBRhJeqcWgR
			hex!["38f3c2f38f6d47f161e98c697bbe3ca0e47c033460afda0dda314ab4222a0404"].into(),
			//5GGdKNDr9P47dpVnmtq3m8Tvowwf1ot1abw6tPsTYYFoKm2v
			hex!["ba0898c1964196474c0be08d364cdf4e9e1d47088287f5235f70b0590dfe1704"].into(),
			//5EjkyPCzR2SjhDZq8f7ufsw6TfkvgNRepjCRQFc4TcdXdaB1
			hex!["764186bc30fd5a02477f19948dc723d6d57ab174debd4f80ed6038ec960bfe21"]
				.unchecked_into(),
			//5DJV3zCBTJBLGNDCcdWrYxWDacSz84goGTa4pFeKVvehEBte
			hex!["36be9069cdb4a8a07ecd51f257875150f0a8a1be44a10d9d98dabf10a030aef4"]
				.unchecked_into(),
			//5FHf8kpK4fPjEJeYcYon2gAPwEBubRvtwpzkUbhMWSweKPUY
			hex!["8e95b9b5b4dc69790b67b566567ca8bf8cdef3a3a8bb65393c0d1d1c87cd2d2c"]
				.unchecked_into(),
			//5F9FsRjpecP9GonktmtFL3kjqNAMKjHVFjyjRdTPa4hbQRZA
			hex!["882d72965e642677583b333b2d173ac94b5fd6c405c76184bb14293be748a13b"]
				.unchecked_into(),
			//5F1FZWZSj3JyTLs8sRBxU6QWyGLSL9BMRtmSKDmVEoiKFxSP
			hex!["821271c99c958b9220f1771d9f5e29af969edfa865631dba31e1ab7bc0582b75"]
				.unchecked_into(),
			//5CtgRR74VypK4h154s369abs78hDUxZSJqcbWsfXvsjcHJNA
			hex!["2496f28d887d84705c6dae98aee8bf90fc5ad10bb5545eca1de6b68425b70f7c"]
				.unchecked_into(),
			//5CPx6dsr11SCJHKFkcAQ9jpparS7FwXQBrrMznRo4Hqv1PXz
			hex!["0307d29bbf6a5c4061c2157b44fda33b7bb4ec52a5a0305668c74688cedf288d58"]
				.unchecked_into(),
		),
		(
			//5C8AL1Zb4bVazgT3EgDxFgcow1L4SJjVu44XcLC9CrYqFN4N
			hex!["02a2d8cfcf75dda85fafc04ace3bcb73160034ed1964c43098fb1fe831de1b16"].into(),
			//5FLYy3YKsAnooqE4hCudttAsoGKbVG3hYYBtVzwMjJQrevPa
			hex!["90cab33f0bb501727faa8319f0845faef7d31008f178b65054b6629fe531b772"].into(),
			//5Et3tfbVf1ByFThNAuUq5pBssdaPPskip5yob5GNyUFojXC7
			hex!["7c94715e5dd8ab54221b1b6b2bfa5666f593f28a92a18e28052531de1bd80813"]
				.unchecked_into(),
			//5EX1JBghGbQqWohTPU6msR9qZ2nYPhK9r3RTQ2oD1K8TCxaG
			hex!["6c878e33b83c20324238d22240f735457b6fba544b383e70bb62a27b57380c81"]
				.unchecked_into(),
			//5GqL8RbVAuNXpDhjQi1KrS1MyNuKhvus2AbmQwRGjpuGZmFu
			hex!["d2f9d537ffa59919a4028afdb627c14c14c97a1547e13e8e82203d2049b15b1a"]
				.unchecked_into(),
			//5EUNaBpX9mJgcmLQHyG5Pkms6tbDiKuLbeTEJS924Js9cA1N
			hex!["6a8570b9c6408e54bacf123cc2bb1b0f087f9c149147d0005badba63a5a4ac01"]
				.unchecked_into(),
			//5CaZuueRVpMATZG4hkcrgDoF4WGixuz7zu83jeBdY3bgWGaG
			hex!["16c69ea8d595e80b6736f44be1eaeeef2ac9c04a803cc4fd944364cb0d617a33"]
				.unchecked_into(),
			//5DABsdQCDUGuhzVGWe5xXzYQ9rtrVxRygW7RXf9Tsjsw1aGJ
			hex!["306ac5c772fe858942f92b6e28bd82fb7dd8cdd25f9a4626c1b0eee075fcb531"]
				.unchecked_into(),
			//5H91T5mHhoCw9JJG4NjghDdQyhC6L7XcSuBWKD3q3TAhEVvQ
			hex!["02fb0330356e63a35dd930bc74525edf28b3bf5eb44aab9e9e4962c8309aaba6a6"]
				.unchecked_into(),
		),
		(
			//5C8XbDXdMNKJrZSrQURwVCxdNdk8AzG6xgLggbzuA399bBBF
			hex!["02ea6bfa8b23b92fe4b5db1063a1f9475e3acd0ab61e6b4f454ed6ba00b5f864"].into(),
			//5GsyzFP8qtF8tXPSsjhjxAeU1v7D1PZofuQKN9TdCc7Dp1JM
			hex!["d4ffc4c05b47d1115ad200f7f86e307b20b46c50e1b72a912ec4f6f7db46b616"].into(),
			//5GHWB8ZDzegLcMW7Gdd1BS6WHVwDdStfkkE4G7KjPjZNJBtD
			hex!["bab3cccdcc34401e9b3971b96a662686cf755aa869a5c4b762199ce531b12c5b"]
				.unchecked_into(),
			//5GzDPGbUM9uH52ZEwydasTj8edokGUJ7vEpoFWp9FE1YNuFB
			hex!["d9c056c98ca0e6b4eb7f5c58c007c1db7be0fe1f3776108f797dd4990d1ccc33"]
				.unchecked_into(),
			//5GWZbVkJEfWZ7fRca39YAQeqri2Z7pkeHyd7rUctUHyQifLp
			hex!["c4a980da30939d5bb9e4a734d12bf81259ae286aa21fa4b65405347fa40eff35"]
				.unchecked_into(),
			//5CmLCFeSurRXXtwMmLcVo7sdJ9EqDguvJbuCYDcHkr3cpqyE
			hex!["1efc23c0b51ad609ab670ecf45807e31acbd8e7e5cb7c07cf49ee42992d2867c"]
				.unchecked_into(),
			//5DnsSy8a8pfE2aFjKBDtKw7WM1V4nfE5sLzP15MNTka53GqS
			hex!["4c64d3f06d28adeb36a892fdaccecace150bec891f04694448a60b74fa469c22"]
				.unchecked_into(),
			//5CZdFnyzZvKetZTeUwj5APAYskVJe4QFiTezo5dQNsrnehGd
			hex!["160ea09c5717270e958a3da42673fa011613a9539b2e4ebcad8626bc117ca04a"]
				.unchecked_into(),
			//5HgoR9JJkdBusxKrrs3zgd3ToppgNoGj1rDyAJp4e7eZiYyT
			hex!["020019a8bb188f8145d02fa855e9c36e9914457d37c500e03634b5223aa5702474"]
				.unchecked_into(),
		),
		(
			//5HinEonzr8MywkqedcpsmwpxKje2jqr9miEwuzyFXEBCvVXM
			hex!["fa373e25a1c4fe19c7148acde13bc3db1811cf656dc086820f3dda736b9c4a00"].into(),
			//5EHJbj6Td6ks5HDnyfN4ttTSi57osxcQsQexm7XpazdeqtV7
			hex!["62145d721967bd88622d08625f0f5681463c0f1b8bcd97eb3c2c53f7660fd513"].into(),
			//5EeCsC58XgJ1DFaoYA1WktEpP27jvwGpKdxPMFjicpLeYu96
			hex!["720537e2c1c554654d73b3889c3ef4c3c2f95a65dd3f7c185ebe4afebed78372"]
				.unchecked_into(),
			//5DnEySxbnppWEyN8cCLqvGjAorGdLRg2VmkY96dbJ1LHFK8N
			hex!["4bea0b37e0cce9bddd80835fa2bfd5606f5dcfb8388bbb10b10c483f0856cf14"]
				.unchecked_into(),
			//5E1Y1FJ7dVP7qtE3wm241pTm72rTMcDT5Jd8Czv7Pwp7N3AH
			hex!["560d90ca51e9c9481b8a9810060e04d0708d246714960439f804e5c6f40ca651"]
				.unchecked_into(),
			//5CAC278tFCHAeHYqE51FTWYxHmeLcENSS1RG77EFRTvPZMJT
			hex!["042f07fc5268f13c026bbe199d63e6ac77a0c2a780f71cda05cee5a6f1b3f11f"]
				.unchecked_into(),
			//5HjRTLWcQjZzN3JDvaj1UzjNSayg5ZD9ZGWMstaL7Ab2jjAa
			hex!["fab485e87ed1537d089df521edf983a777c57065a702d7ed2b6a2926f31da74f"]
				.unchecked_into(),
			//5ELv74v7QcsS6FdzvG4vL2NnYDGWmRnJUSMKYwdyJD7Xcdi7
			hex!["64d59feddb3d00316a55906953fb3db8985797472bd2e6c7ea1ab730cc339d7f"]
				.unchecked_into(),
			//5FaUcPt4fPz93vBhcrCJqmDkjYZ7jCbzAF56QJoCmvPaKrmx
			hex!["033f1a6d47fe86f88934e4b83b9fae903b92b5dcf4fec97d5e3e8bf4f39df03685"]
				.unchecked_into(),
		),
		(
			//5Ey3NQ3dfabaDc16NUv7wRLsFCMDFJSqZFzKVycAsWuUC6Di
			hex!["8062e9c21f1d92926103119f7e8153cebdb1e5ab3e52d6f395be80bb193eab47"].into(),
			//5HiWsuSBqt8nS9pnggexXuHageUifVPKPHDE2arTKqhTp1dV
			hex!["fa0388fa88f3f0cb43d583e2571fbc0edad57dff3a6fd89775451dd2c2b8ea00"].into(),
			//5H168nKX2Yrfo3bxj7rkcg25326Uv3CCCnKUGK6uHdKMdPt8
			hex!["da6b2df18f0f9001a6dcf1d301b92534fe9b1f3ccfa10c49449fee93adaa8349"]
				.unchecked_into(),
			//5DrA2fZdzmNqT5j6DXNwVxPBjDV9jhkAqvjt6Us3bQHKy3cF
			hex!["4ee66173993dd0db5d628c4c9cb61a27b76611ad3c3925947f0d0011ee2c5dcc"]
				.unchecked_into(),
			//5FNFDUGNLUtqg5LgrwYLNmBiGoP8KRxsvQpBkc7GQP6qaBUG
			hex!["92156f54a114ee191415898f2da013d9db6a5362d6b36330d5fc23e27360ab66"]
				.unchecked_into(),
			//5Gx6YeNhynqn8qkda9QKpc9S7oDr4sBrfAu516d3sPpEt26F
			hex!["d822d4088b20dca29a580a577a97d6f024bb24c9550bebdfd7d2d18e946a1c7d"]
				.unchecked_into(),
			//5DhDcHqwxoes5s89AyudGMjtZXx1nEgrk5P45X88oSTR3iyx
			hex!["481538f8c2c011a76d7d57db11c2789a5e83b0f9680dc6d26211d2f9c021ae4c"]
				.unchecked_into(),
			//5DqAvikdpfRdk5rR35ZobZhqaC5bJXZcEuvzGtexAZP1hU3T
			hex!["4e262811acdfe94528bfc3c65036080426a0e1301b9ada8d687a70ffcae99c26"]
				.unchecked_into(),
			//5E41Znrr2YtZu8bZp3nvRuLVHg3jFksfQ3tXuviLku4wsao7
			hex!["025e84e95ed043e387ddb8668176b42f8e2773ddd84f7f58a6d9bf436a4b527986"]
				.unchecked_into(),
		),
	];

	const ENDOWMENT: u128 = 1_000_000 * ROC;
	const STASH: u128 = 100 * ROC;

	rococo_runtime::GenesisConfig {
		system: rococo_runtime::SystemConfig { code: wasm_binary.to_vec() },
		balances: rococo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		},
		beefy: Default::default(),
		indices: rococo_runtime::IndicesConfig { indices: vec![] },
		session: rococo_runtime::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						rococo_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
							x.8.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		babe: rococo_runtime::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(rococo_runtime::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		collective: Default::default(),
		membership: Default::default(),
		authority_discovery: rococo_runtime::AuthorityDiscoveryConfig { keys: vec![] },
		sudo: rococo_runtime::SudoConfig { key: Some(endowed_accounts[0].clone()) },
		paras: rococo_runtime::ParasConfig { paras: vec![] },
		hrmp: Default::default(),
		configuration: rococo_runtime::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		registrar: rococo_runtime::RegistrarConfig {
			next_free_para_id: polkadot_primitives::v2::LOWEST_PUBLIC_ID,
		},
		xcm_pallet: Default::default(),
		transaction_payment: Default::default(),
		bridge_rococo_grandpa: rococo_runtime::BridgeRococoGrandpaConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_wococo_grandpa: rococo_runtime::BridgeWococoGrandpaConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_rococo_messages: rococo_runtime::BridgeRococoMessagesConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_wococo_messages: rococo_runtime::BridgeWococoMessagesConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
	}
}

#[cfg(feature = "rococo-native")]
fn versi_staging_testnet_config_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	use hex_literal::hex;
	use sp_core::crypto::UncheckedInto;

	// subkey inspect "$SECRET"
	let endowed_accounts = vec![
		// 5DwBmEFPXRESyEam5SsQF1zbWSCn2kCjyLW51hJHXe9vW4xs
		hex!["52bc71c1eca5353749542dfdf0af97bf764f9c2f44e860cd485f1cd86400f649"].into(),
	];

	// ./scripts/prepare-test-net.sh 7 (i = versi-bootnode-$i)
	let initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
		BeefyId,
	)> = vec![
		(
			//versi-bootnode-0
			//5DV3X8jaZ2pwUFnApusxSQc3hPZrbCKByvdGfLvwFqcKXRte
			hex!["3ecc50bca30f045f94cc609ea3acf859ec2f2b93abd551e48394fa227ecb213d"].into(),
			//5Ey8RkgbfT2aJyXjHg9KgUxDVbY64LGdx328compLvRE3fzG
			hex!["8073f07bcddc9bd1542a7d3ea80fb6b5638e4589645e0da07a43910c4dcee85d"].into(),
			//5FNF9A2TFd8PgutZd8bdPpbR1WSZCcjtnggd9EEq3oVKVM36
			hex!["92152f39b165099df481ab7780f375f496fada5544e8b66cab67df4c493f3128"].unchecked_into(),
			//5GL8faNkBoJS6TeZANDGjo8PcPyPs5WZQ85J5uB4jqPpyvh8
			hex!["bcb520ea39e9bf69cb47b4dda167dafde1da01f01bb7d2c091ab50fb7bf8f1d7"].unchecked_into(),
			//5EycsfQxcsFBjJ6setX8yivnceNiBoZtiQkcryvNQA9K4ULK
			hex!["80d3b315a556758b44a2bf7467c2320e5840693c502ea686a89de6fc09c6ff38"].unchecked_into(),
			//5DjqDw5NMmFdiafR9oVKZLasjY96jsd4sM5sdhMi8CT29vKx
			hex!["4a139a3d919f44ba4709d65c9bd5e1c3c9392b4572e7e1cb28c73bd7591ad51c"].unchecked_into(),
			//5F7Ka1qtTNVLityecYunrePHew3xCzV8HwZcrGZ9fuCS9NWv
			hex!["86b36952561d89ba10ffc70d0daffcd6fdd55899dfe5c6e051135087b6b1e30b"].unchecked_into(),
			//5Hb3UnjMkuTWN47mKFxFfrad3gG8QzVNHogq3Nmj9drmnQPZ
			hex!["f45095e49de853e229bb15c402483d7633a5eba25da2c38e4fd3a94df4550151"].unchecked_into(),
			//5EEFWe2ssjEToHmFcj8wCpbZxf8HPRVmDVKzjzwHmigDobTR
			hex!["02dcfd39de094705787507db7a674840350592d9dd33c89256444915d9c35bd541"].unchecked_into(),
		),
		(
			//versi-bootnode-1
			//5Cag9DRBPYVHdpT4jTeciPeSikrFXmo6RvM7tvWTQgbtmVz8
			hex!["16db97014b8c694fc3d7651d05f114d28dd3d106cddc5086738299709540c606"].into(),
			//5D51C2P7GMEyQCfxLiQz77DrkDiywnX21kKkwjskBNWWLxwv
			hex!["2c76957e95c326c576780c1b9628bc69b1862ea66e5fd614ddafe3a0dcc13d42"].into(),
			//5FLny6BTcXSWMjZHgaiEbh5GPgyoTuDjqmd4PCCakKDPyYQL
			hex!["90f9d4bdfa86aa74e7ae0540604a30001e7161ea129b10790168aa82cf140217"].unchecked_into(),
			//5CpyzFD8PePkdLNCpMkGSoHp5xQy4bNoga4fgSV3gik9A5gV
			hex!["21c5192e4837d49aaa84978cf50b042a7fefb6859a9ec36dbf31e47afa251106"].unchecked_into(),
			//5HB5z5q5d2AdeT6EiQHd4KTweUr7m3KTZfvcMVJPBZfwh98v
			hex!["e20b20f0e0e9ef3e7067cb686f5e863fd199ea9d15278f5c5a3f4e996c6b1b01"].unchecked_into(),
			//5FTqYsXezAyeKREwBwMjrQURFMZ9VSVWLom4TNo514aD9F7A
			hex!["96593dfd0b6a3df882e9e59f72254d510b5b3bc8426a14519c52e10366331220"].unchecked_into(),
			//5HGhpDMW91xYabtKr29vUTuJqtsz1rucuEKiRz5cMUfYKVeG
			hex!["e653f839e0a45f87ff64be931d88c2c675027f97f78db1dbc4e5c3b177e68a67"].unchecked_into(),
			//5HKqyEHGaAo7zZYxeY5Q4Ti1B2ooXKYvvPBzjNXan3Fpz8z2
			hex!["e8b928d70c45e2bec1d701e3bb74fed373848759edf8038780f2391233f0fd56"].unchecked_into(),
			//5FezXdtwcWVutVyGJm498mWF4PpR3RRNjrpEYt7vLMrUaq11
			hex!["0386f6aced2d95c885ef955f2309a377546e6d3b05ec65e3c0fe1b60b51780d9cb"].unchecked_into(),
		),
		(
			//versi-bootnode-2
			//5GYgFK92vZj833faADWvFvtfQNEMKJruA6hfnQAb3Pwj9CMU
			hex!["c64663074d7725f30a8c3fcc199758a4ee663c1c17ffe3043019b70e7b25281f"].into(),
			//5GYkr1M6niodK8wnfotoNyuyMfT8wkT7tiBBj7enkLDjg1kr
			hex!["c655dd98e6b25308042cc7e2a4089b0771ac9706f093714b1752f7186cdff903"].into(),
			//5EWkYHRWeLtsZ9ZZcTLt4hvsZ7vFeCyu5N8SFGvghNWN9BMX
			hex!["6c55e1180722f0a2d03a52cd84e759e7bfc2ee0cf3d639557013e8f191534503"].unchecked_into(),
			//5ExhjXhUcbD6BDDS4B4EUoDkxTWxuh2eADJS9XkrRMBSfp4a
			hex!["8020d0489180c87dd10ff6bc18dc4ff0b5b5561f232782c6f81834cd61ce6d37"].unchecked_into(),
			//5F7YM3nbu6MpYpaLhnTr44dyvkte1T6T5ZPxznRPkL6NA5JY
			hex!["86de6bb5cba98095c96ce82161f28f1d5f95a143167512f5796ea816d572fb35"].unchecked_into(),
			//5DyXfRM9Q7y9ai88iRwUL9PhGsFu1qsuXkUhV96HyN2QzPB5
			hex!["5485ec8e2cf71922706e2276a9e3bcce1e1fd0719d987a2948cf7fbdf6d23e18"].unchecked_into(),
			//5Hp5F5ANNvzfAPrAKjMAervCzkrjBTBD1G8FosrVAtAvsA5K
			hex!["fe40b656eb50bf060345437f1cf1443ccdb38b757dc16fcdcec38f3db624cb19"].unchecked_into(),
			//5FjR5ZGcphJwa8ASbN4Zy7G68CBSYncMCvnzb8NKjgavRu8c
			hex!["a23acac4f679a303340473c47cf06dba6dd8cb25c31e4a198ac03198cb03bb0e"].unchecked_into(),
			//5CuBp8GJhoEGPsqP7hxbMZFbvfqCXPtfGGqHyPg4TA6VjYZm
			hex!["0342fa2e397cc25068d5264a9cddeec09eb7649e204b0fcff16bf72ec2a47121c3"].unchecked_into(),
		),
		(
			//versi-bootnode-3
			//5F7EdNXunchNw4DnsSqXnhMZXLQRxuo1my2Ea86suAJYyzLE
			hex!["86a2c6550f0e7372bbe64ad6e8bd4417dac9895278036a6ddbce442dde507178"].into(),
			//5GCTSeDTPkTYb2wDgqBNfZCHyrsPNrQ7o3gaqxhBisxw2y1j
			hex!["b6da5fd68be99a24ee305870cbb4dbfbbf990b2c136fef5b86fde9ac3484027e"].into(),
			//5Dqqu4PZ6CEfLmuHMq1Zh445sNoWMyGJ2koKDvViJaSNSym1
			hex!["4ea95885e872aa9d0f1e0f02585d87d9014fcd0ee3c675d841adf332b328dd76"].unchecked_into(),
			//5G112ibbo5RpVoEZzfk925XC5ZhudgGGcVp89EtjH8p4sgSd
			hex!["ae1dc33683dfc96f85041e45acef7bcbe689bdaca2e6de30921e67cae9f04466"].unchecked_into(),
			//5G8uMor82Z361AL26srzShBJPcZW2WeBWnyQ8zY8HeVT8iDc
			hex!["b424a2d066d145ca99b33272102629e03f2de893f18eab1a4a7e00d82e99084e"].unchecked_into(),
			//5Cvs52pMzW6ygaKuy9PqBuqd486aaXEi3jGrdAvSPcBqAuMN
			hex!["264148d4090f3e25889513c2fabc16a1f5868beec84a416587f67bb7408ff47a"].unchecked_into(),
			//5CV7RehhbrMXhRy5ULMLz1Z6K9phjAHTh7NciS32PAxcdhUS
			hex!["129d3a979bad7850a525b3fe2941f70a9405657fdd213819fba209cf00206434"].unchecked_into(),
			//5CvZfiLpvRFXnL3TnQ6q174AaKU5ZbEEApDfXqyfwTZthpxp
			hex!["2606b43856617293a0f9743979a812c18d6dd3940aa5cb42765385d925165b51"].unchecked_into(),
			//5EwUULMqszegdrs8Papyu2goesMzQqiRVFd5HBo95KabrJn3
			hex!["03cdaac2af6282482e37761f74cd86f70b0dff46faa38fea38babec5c639113ea0"].unchecked_into(),
		),
		(
			//versi-bootnode-4
			//5Enyc7KFiNo3cya5nrDe4iwMGxMFxcJHGqAcraKnnrEocL8T
			hex!["78b5ca28f56ae0022fa876d5ced684e18ed1afdb4fa1e6da5c1d26389ad9a434"].into(),
			//5H8hp25gnTvVuePYD5DkYnzS3nSuWm3y9tdrh1omFj8U8wxm
			hex!["e039fec00afaebd9af15014440e1febe07a2effa1c72bfca738ca770c5eb177f"].into(),
			//5H3Yis5m8ikyU8nUHGYNfvvPyFtHPDb19KGTxYhweznNJHUH
			hex!["dc4b2c14c9f977b976354aeee6e1dfc849b5757eea2e11f7dc6fe4e0a34d4939"].unchecked_into(),
			//5Dp77hZ1bG4kdkVa2LR44dZqCDrLDfgfRpSygZTCVrhHuFjX
			hex!["4d56154951c76353b204066dbff0fec9b335854a1608f8d4ca04375c78adfb39"].unchecked_into(),
			//5D5aY6RacUAtLQPRJLNfYUogDmJxQKpuRfGSm75mFLU25cdM
			hex!["2ce6d63237e54cb99d830b5fdf7ad097d5954a0102b77527211b337cda84ca09"].unchecked_into(),
			//5Hpvfs89hBjqEF1cXUiKZXBv4no1oC2mHw1NpQ11C6XCgMGW
			hex!["fee71993dbcfc41bd6a053ab9f9705f377c71b875f8abcbb2081c241f9f3a31e"].unchecked_into(),
			//5EPXP8yv65npdoSig3ru22XXS4CYeK1W5P2pGXrTSu13CnUo
			hex!["66d2de6592d6ec8093a7ef738595e70174306e3d5399306a114e41348b218c3f"].unchecked_into(),
			//5HGTYBcTgosXkVGUchpPT2VYi5QEhYNbYgUdXhX1NQwpAYGD
			hex!["e623e93ada87315b9b67125577bcf38d63351eba661767e8d4c7f82de214bb42"].unchecked_into(),
			//5EbCRa9Bhjy2b5MDcSAUXZJaNgZSQG8BHFFDoEVKcrrTmNib
			hex!["030cc28a18b53affcc5983e0e4e2f874bd81af4da90243110e7ee685661d90dd15"].unchecked_into(),
		),
		(
			//versi-bootnode-5
			//5HisWy38kugc3Ay4XAsrg1DAU3pJfsQhmPToY5Y726ZbunV8
			hex!["fa49031c0aef3f7c987ff7c1511f36ff1bd5545aaa9cbec76d939b617fae6f5d"].into(),
			//5ERiUsDFC7W25CPJ7nZXQRa1hCNW3z5wdMNWeEfNJLQgVHuU
			hex!["687eb8a75f901f5484d59aebab21d9738f8423f74748144c4d8a90a57220de31"].into(),
			//5FsX7MZEXD8PV4VMKitxHJDQHaw9aufVYDeHj9TnSZMxh19Z
			hex!["a8690eb5c4394f2dac264ff2a970009c4807cc3ea1c23e5b8b1b522e3bc94617"].unchecked_into(),
			//5E2BMeeNPfHoaoeGkRcwuSXmHxQKt7kkVWwzzg1EyeM9Uxsq
			hex!["568b4d8776812f8314825dab808aab07dd98019b60e3b897946df389507b89ac"].unchecked_into(),
			//5GNT2rvcVbiyXoBEFCEmyuKPHCeyU7t6Z9b9P9QZZQt5zkyP
			hex!["be7972bbc4ce359af881678ee1ccdcbc1b649bcbc6f82c9c849c051bac818f03"].unchecked_into(),
			//5CDkpJJQDfjwrhWG6mHMQehLLvykkqcutcU8hyv5qR5cxeAs
			hex!["06e72b8577559c32b628b1ec4b1ee1c75b2fc6ba02b286f569b96e3055a6b759"].unchecked_into(),
			//5FgrEDFDrbmghpVaeg59cy5hsRk5bNpXcKBy9sCg8STa9KZ8
			hex!["a045b736887076556955e5ad890b074734af90d704e1efaf3b6c8713d90b0d27"].unchecked_into(),
			//5GTFG9KW6j628acwSV9GgjNxMRenUihEvBGfoz8L74EGog9t
			hex!["c2220ceedfb0bc06b2b66023beec32efb9ec2be9c1f17de34b66ed8188322e34"].unchecked_into(),
			//5DebHv79jFE6WPrTPyc5Ks7yzcYrQCnqJ16k6F2CCqXJDLQt
			hex!["035a4034a2786140da49c5568609ca1e82d1a7a1d526cdc0eb0c0e467d53522c5d"].unchecked_into(),
		),
		(
			//versi-bootnode-6
			//5CkxZrGeGfpQApMw6HAtBF9xiPgxoA3Ei7xZ69aRry6iGzNL
			hex!["1eb355a44a059486fb38a3ef359f315c84d078f46fca3abf8a7a12a68ef69630"].into(),
			//5CofQrypXYMKDygCjfshuaTeN92NhRh4kApgjnK99rgCm1Aq
			hex!["20c3526ed124f9119185c079d6670461c88e04d76f1ef2cb38b02ff3c24f081a"].into(),
			//5Fv73X7dtFm15hqYG338MNJ1qw351BbgjzFAh3rgXdjDFaCT
			hex!["aa61c7a624b0289509c41a7f76a28193fe5c84ec21a55a5068529c39f8797e31"].unchecked_into(),
			//5EvP4zKAJDnj7JSomquk2pVRucYG6vvcepf6b6Hpjv95hKHf
			hex!["7e5b7e136f9836ce5e0eaf44ea234de57843f71401c2d769987480f96558211b"].unchecked_into(),
			//5EcGLLCftCN9zGM3sB769kJfZrPDsdvrSskUubGhP8nCH6Zr
			hex!["708a66cf5c7af82c651ecdb9be402af15d867e9c17ac9dfac7db7da1945ae20c"].unchecked_into(),
			//5DAgS6rUfqrVec19jTSZkMFJdQWLUT3WtnvdnHBmigYWadSk
			hex!["30cae9e96670adf8ee5eefcdeeea68d72100a1001bb4a585783500dccd47fc5d"].unchecked_into(),
			//5HMfW3gxehXWQzTs7xLcau5jFe5448PB7BzJKzTTUNPbN6xc
			hex!["ea1c68e01f6c3fcf125ff1829bfb0783ce593fc2c0a208a7ac256c2f959df87c"].unchecked_into(),
			//5CrF49SzojrG99QMbSJfQAuqaC8Ui23mk8V4crWwfZ2tbA68
			hex!["22bb108dac5180f543a64ee46b3156538c0ab4c8f683ea692f7ffdebd49ec464"].unchecked_into(),
			//5CYFbx7uC7voQh7otWP9EMyCR62GxxNySNpJhFkUtoBe3VVG
			hex!["037634779c308fb279373f1399fe6ba11c980913103417db3d4a910a72d9f5f95d"].unchecked_into(),
		),
		(
			//versi-bootnode-7
			//5EyVyQG5L4zY9xTrpqrH4NYFeTxz6Nv7QVi7mAKxashEDaDn
			hex!["80bc77f11e1eeb294548f05fc00b3069aac59b8281f85eb22515c954bededf52"].into(),
			//5FYk4J9j7Vce4fLLQQCb7RwdgrXbZ4rBrPPuak8rwaZhDXyt
			hex!["9a16faa9bdd42666484f587f5f3ba9b3d24c0e9f66e1a602a91e3efbdb250f65"].into(),
			//5DLugVPwnwTdpxcoL54UVrJy22mBWR1ojy6ft589NSm23jSB
			hex!["3897f6fb853c16ef740ca9c287e82fadaa22a432e9652ce9fe79d1c3cc884d43"].unchecked_into(),
			//5Ddf9e2CRXeqFaJ7xqvndZwGTY3waxPXCEdYgR81gYfyMTwz
			hex!["455e37c505d65a7c2ed94277a886457eed79f54e90e4982224f9c2d25eec3afb"].unchecked_into(),
			//5Eh7u54DiiGH4QUj8sbkL8gDgW5vMh7SP5JFVa8CBMCQvkqi
			hex!["743e3b6949a62ecd51275790a3bf288266e71bb4c4296c7d097bbed18b4e0f6c"].unchecked_into(),
			//5HontyHjLF3jQD7sqwKbKn1u3oFDPX2aoiqnQw1UGDBT1MKc
			hex!["fe09af4e8d343fedabd0399dfb879d61c17d5144efb9318ee6cd19bc9419ea0c"].unchecked_into(),
			//5CSyR1g4pHHzQ7ppafUhXcg6RiqNJpsN5sbrVLnCfX9DCCUp
			hex!["10fbc546e5615fa403516302ba67ff5139cbf2d6e289e9dbb77b41e1bd643b71"].unchecked_into(),
			//5HGaFebZR87Yz94GA52uc8eb7k9nLwEqjqdmYUAfk9SveXah
			hex!["e63a83f6bb16b18a42e8f4df6ff4e93f0b9b56f0339963ef43e6eb6a84723122"].unchecked_into(),
			//5EZ1QLHJM6R2Z7tafMREmjnEpTWNozsq2YQsVhRDQu1KWEHM
			hex!["02a11607acf8e14511325c451468c2671514280349929bc0924e18da23f33677de"].unchecked_into(),
		),
	];

	const ENDOWMENT: u128 = 1_000_000 * ROC;
	const STASH: u128 = 100 * ROC;

	rococo_runtime::GenesisConfig {
		system: rococo_runtime::SystemConfig { code: wasm_binary.to_vec() },
		balances: rococo_runtime::BalancesConfig {
			balances: endowed_accounts
				.iter()
				.map(|k: &AccountId| (k.clone(), ENDOWMENT))
				.chain(initial_authorities.iter().map(|x| (x.0.clone(), STASH)))
				.collect(),
		},
		beefy: Default::default(),
		indices: rococo_runtime::IndicesConfig { indices: vec![] },
		session: rococo_runtime::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						rococo_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
							x.8.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		babe: rococo_runtime::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(rococo_runtime::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		collective: Default::default(),
		membership: Default::default(),
		authority_discovery: rococo_runtime::AuthorityDiscoveryConfig { keys: vec![] },
		sudo: rococo_runtime::SudoConfig { key: Some(endowed_accounts[0].clone()) },
		paras: rococo_runtime::ParasConfig { paras: vec![] },
		hrmp: Default::default(),
		configuration: rococo_runtime::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		registrar: rococo_runtime::RegistrarConfig {
			next_free_para_id: polkadot_primitives::v2::LOWEST_PUBLIC_ID,
		},
		xcm_pallet: Default::default(),
		transaction_payment: Default::default(),
		bridge_rococo_grandpa: rococo_runtime::BridgeRococoGrandpaConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_wococo_grandpa: rococo_runtime::BridgeWococoGrandpaConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_rococo_messages: rococo_runtime::BridgeRococoMessagesConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
		bridge_wococo_messages: rococo_runtime::BridgeWococoMessagesConfig {
			owner: Some(endowed_accounts[0].clone()),
			..Default::default()
		},
	}
}

/// Returns the properties for the [`PolkadotChainSpec`].
pub fn polkadot_chain_spec_properties() -> serde_json::map::Map<String, serde_json::Value> {
	serde_json::json!({
		"tokenDecimals": 10,
	})
	.as_object()
	.expect("Map given; qed")
	.clone()
}

/// Polkadot staging testnet config.
#[cfg(feature = "polkadot-native")]
pub fn polkadot_staging_testnet_config() -> Result<PolkadotChainSpec, String> {
	let wasm_binary = polkadot::WASM_BINARY.ok_or("Polkadot development wasm not available")?;
	let boot_nodes = vec![];

	Ok(PolkadotChainSpec::from_genesis(
		"Polkadot Staging Testnet",
		"polkadot_staging_testnet",
		ChainType::Live,
		move || polkadot_staging_testnet_config_genesis(wasm_binary),
		boot_nodes,
		Some(
			TelemetryEndpoints::new(vec![(POLKADOT_STAGING_TELEMETRY_URL.to_string(), 0)])
				.expect("Polkadot Staging telemetry url is valid; qed"),
		),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		Some(polkadot_chain_spec_properties()),
		Default::default(),
	))
}

/// Staging testnet config.
#[cfg(feature = "kusama-native")]
pub fn kusama_staging_testnet_config() -> Result<KusamaChainSpec, String> {
	let wasm_binary = kusama::WASM_BINARY.ok_or("Kusama development wasm not available")?;
	let boot_nodes = vec![];

	Ok(KusamaChainSpec::from_genesis(
		"Kusama Staging Testnet",
		"kusama_staging_testnet",
		ChainType::Live,
		move || kusama_staging_testnet_config_genesis(wasm_binary),
		boot_nodes,
		Some(
			TelemetryEndpoints::new(vec![(KUSAMA_STAGING_TELEMETRY_URL.to_string(), 0)])
				.expect("Kusama Staging telemetry url is valid; qed"),
		),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// Westend staging testnet config.
#[cfg(feature = "westend-native")]
pub fn westend_staging_testnet_config() -> Result<WestendChainSpec, String> {
	let wasm_binary = westend::WASM_BINARY.ok_or("Westend development wasm not available")?;
	let boot_nodes = vec![];

	Ok(WestendChainSpec::from_genesis(
		"Westend Staging Testnet",
		"westend_staging_testnet",
		ChainType::Live,
		move || westend_staging_testnet_config_genesis(wasm_binary),
		boot_nodes,
		Some(
			TelemetryEndpoints::new(vec![(WESTEND_STAGING_TELEMETRY_URL.to_string(), 0)])
				.expect("Westend Staging telemetry url is valid; qed"),
		),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// Rococo staging testnet config.
#[cfg(feature = "rococo-native")]
pub fn rococo_staging_testnet_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Rococo development wasm not available")?;
	let boot_nodes = vec![];

	Ok(RococoChainSpec::from_genesis(
		"Rococo Staging Testnet",
		"rococo_staging_testnet",
		ChainType::Live,
		move || RococoGenesisExt {
			runtime_genesis_config: rococo_staging_testnet_config_genesis(wasm_binary),
			session_length_in_blocks: None,
		},
		boot_nodes,
		Some(
			TelemetryEndpoints::new(vec![(ROCOCO_STAGING_TELEMETRY_URL.to_string(), 0)])
				.expect("Rococo Staging telemetry url is valid; qed"),
		),
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

pub fn versi_chain_spec_properties() -> serde_json::map::Map<String, serde_json::Value> {
	serde_json::json!({
		"ss58Format": 42,
		"tokenDecimals": 12,
		"tokenSymbol": "VRS",
	})
	.as_object()
	.expect("Map given; qed")
	.clone()
}

/// Versi staging testnet config.
#[cfg(feature = "rococo-native")]
pub fn versi_staging_testnet_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Versi development wasm not available")?;
	let boot_nodes = vec![];

	Ok(RococoChainSpec::from_genesis(
		"Versi Staging Testnet",
		"versi_staging_testnet",
		ChainType::Live,
		move || RococoGenesisExt {
			runtime_genesis_config: versi_staging_testnet_config_genesis(wasm_binary),
			session_length_in_blocks: Some(100),
		},
		boot_nodes,
		Some(
			TelemetryEndpoints::new(vec![(VERSI_STAGING_TELEMETRY_URL.to_string(), 0)])
				.expect("Versi Staging telemetry url is valid; qed"),
		),
		Some("versi"),
		None,
		Some(versi_chain_spec_properties()),
		Default::default(),
	))
}

/// Helper function to generate a crypto pair from seed
pub fn get_from_seed<TPublic: Public>(seed: &str) -> <TPublic::Pair as Pair>::Public {
	TPublic::Pair::from_string(&format!("//{}", seed), None)
		.expect("static values are valid; qed")
		.public()
}

/// Helper function to generate an account ID from seed
pub fn get_account_id_from_seed<TPublic: Public>(seed: &str) -> AccountId
where
	AccountPublic: From<<TPublic::Pair as Pair>::Public>,
{
	AccountPublic::from(get_from_seed::<TPublic>(seed)).into_account()
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed(
	seed: &str,
) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ImOnlineId,
	ValidatorId,
	AssignmentId,
	AuthorityDiscoveryId,
	BeefyId,
) {
	let keys = get_authority_keys_from_seed_no_beefy(seed);
	(keys.0, keys.1, keys.2, keys.3, keys.4, keys.5, keys.6, keys.7, get_from_seed::<BeefyId>(seed))
}

/// Helper function to generate stash, controller and session key from seed
pub fn get_authority_keys_from_seed_no_beefy(
	seed: &str,
) -> (
	AccountId,
	AccountId,
	BabeId,
	GrandpaId,
	ImOnlineId,
	ValidatorId,
	AssignmentId,
	AuthorityDiscoveryId,
) {
	(
		get_account_id_from_seed::<sr25519::Public>(&format!("{}//stash", seed)),
		get_account_id_from_seed::<sr25519::Public>(seed),
		get_from_seed::<BabeId>(seed),
		get_from_seed::<GrandpaId>(seed),
		get_from_seed::<ImOnlineId>(seed),
		get_from_seed::<ValidatorId>(seed),
		get_from_seed::<AssignmentId>(seed),
		get_from_seed::<AuthorityDiscoveryId>(seed),
	)
}

fn testnet_accounts() -> Vec<AccountId> {
	vec![
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		get_account_id_from_seed::<sr25519::Public>("Bob"),
		get_account_id_from_seed::<sr25519::Public>("Charlie"),
		get_account_id_from_seed::<sr25519::Public>("Dave"),
		get_account_id_from_seed::<sr25519::Public>("Eve"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie"),
		get_account_id_from_seed::<sr25519::Public>("Alice//stash"),
		get_account_id_from_seed::<sr25519::Public>("Bob//stash"),
		get_account_id_from_seed::<sr25519::Public>("Charlie//stash"),
		get_account_id_from_seed::<sr25519::Public>("Dave//stash"),
		get_account_id_from_seed::<sr25519::Public>("Eve//stash"),
		get_account_id_from_seed::<sr25519::Public>("Ferdie//stash"),
	]
}

/// Helper function to create polkadot `GenesisConfig` for testing
#[cfg(feature = "polkadot-native")]
pub fn polkadot_testnet_genesis(
	wasm_binary: &[u8],
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)>,
	_root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> polkadot::GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * DOT;
	const STASH: u128 = 100 * DOT;

	polkadot::GenesisConfig {
		system: polkadot::SystemConfig { code: wasm_binary.to_vec() },
		indices: polkadot::IndicesConfig { indices: vec![] },
		balances: polkadot::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		},
		session: polkadot::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						polkadot_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: polkadot::StakingConfig {
			minimum_validator_count: 1,
			validator_count: initial_authorities.len() as u32,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, polkadot::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::NotForcing,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		phragmen_election: Default::default(),
		democracy: polkadot::DemocracyConfig::default(),
		council: polkadot::CouncilConfig { members: vec![], phantom: Default::default() },
		technical_committee: polkadot::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		},
		technical_membership: Default::default(),
		babe: polkadot::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(polkadot::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: polkadot::AuthorityDiscoveryConfig { keys: vec![] },
		claims: polkadot::ClaimsConfig { claims: vec![], vesting: vec![] },
		vesting: polkadot::VestingConfig { vesting: vec![] },
		treasury: Default::default(),
		hrmp: Default::default(),
		configuration: polkadot::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		paras: Default::default(),
		xcm_pallet: Default::default(),
	}
}

/// Helper function to create kusama `GenesisConfig` for testing
#[cfg(feature = "kusama-native")]
pub fn kusama_testnet_genesis(
	wasm_binary: &[u8],
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)>,
	_root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> kusama::GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * KSM;
	const STASH: u128 = 100 * KSM;

	kusama::GenesisConfig {
		system: kusama::SystemConfig { code: wasm_binary.to_vec() },
		indices: kusama::IndicesConfig { indices: vec![] },
		balances: kusama::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		},
		session: kusama::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						kusama_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: kusama::StakingConfig {
			minimum_validator_count: 1,
			validator_count: initial_authorities.len() as u32,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, kusama::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::NotForcing,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		phragmen_election: Default::default(),
		democracy: kusama::DemocracyConfig::default(),
		council: kusama::CouncilConfig { members: vec![], phantom: Default::default() },
		technical_committee: kusama::TechnicalCommitteeConfig {
			members: vec![],
			phantom: Default::default(),
		},
		technical_membership: Default::default(),
		babe: kusama::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(kusama::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: kusama::AuthorityDiscoveryConfig { keys: vec![] },
		claims: kusama::ClaimsConfig { claims: vec![], vesting: vec![] },
		vesting: kusama::VestingConfig { vesting: vec![] },
		treasury: Default::default(),
		hrmp: Default::default(),
		configuration: kusama::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		gilt: Default::default(),
		paras: Default::default(),
		xcm_pallet: Default::default(),
		nomination_pools: Default::default(),
	}
}

/// Helper function to create westend `GenesisConfig` for testing
#[cfg(feature = "westend-native")]
pub fn westend_testnet_genesis(
	wasm_binary: &[u8],
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
	)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> westend::GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * WND;
	const STASH: u128 = 100 * WND;

	westend::GenesisConfig {
		system: westend::SystemConfig { code: wasm_binary.to_vec() },
		indices: westend::IndicesConfig { indices: vec![] },
		balances: westend::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		},
		session: westend::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						westend_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		staking: westend::StakingConfig {
			minimum_validator_count: 1,
			validator_count: initial_authorities.len() as u32,
			stakers: initial_authorities
				.iter()
				.map(|x| (x.0.clone(), x.1.clone(), STASH, westend::StakerStatus::Validator))
				.collect(),
			invulnerables: initial_authorities.iter().map(|x| x.0.clone()).collect(),
			force_era: Forcing::NotForcing,
			slash_reward_fraction: Perbill::from_percent(10),
			..Default::default()
		},
		babe: westend::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(westend::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		authority_discovery: westend::AuthorityDiscoveryConfig { keys: vec![] },
		vesting: westend::VestingConfig { vesting: vec![] },
		sudo: westend::SudoConfig { key: Some(root_key) },
		hrmp: Default::default(),
		configuration: westend::ConfigurationConfig {
			config: default_parachains_host_configuration(),
		},
		paras: Default::default(),
		registrar: westend_runtime::RegistrarConfig {
			next_free_para_id: polkadot_primitives::v2::LOWEST_PUBLIC_ID,
		},
		xcm_pallet: Default::default(),
		nomination_pools: Default::default(),
	}
}

/// Helper function to create rococo `GenesisConfig` for testing
#[cfg(feature = "rococo-native")]
pub fn rococo_testnet_genesis(
	wasm_binary: &[u8],
	initial_authorities: Vec<(
		AccountId,
		AccountId,
		BabeId,
		GrandpaId,
		ImOnlineId,
		ValidatorId,
		AssignmentId,
		AuthorityDiscoveryId,
		BeefyId,
	)>,
	root_key: AccountId,
	endowed_accounts: Option<Vec<AccountId>>,
) -> rococo_runtime::GenesisConfig {
	let endowed_accounts: Vec<AccountId> = endowed_accounts.unwrap_or_else(testnet_accounts);

	const ENDOWMENT: u128 = 1_000_000 * ROC;

	rococo_runtime::GenesisConfig {
		system: rococo_runtime::SystemConfig { code: wasm_binary.to_vec() },
		beefy: Default::default(),
		indices: rococo_runtime::IndicesConfig { indices: vec![] },
		balances: rococo_runtime::BalancesConfig {
			balances: endowed_accounts.iter().map(|k| (k.clone(), ENDOWMENT)).collect(),
		},
		session: rococo_runtime::SessionConfig {
			keys: initial_authorities
				.iter()
				.map(|x| {
					(
						x.0.clone(),
						x.0.clone(),
						rococo_session_keys(
							x.2.clone(),
							x.3.clone(),
							x.4.clone(),
							x.5.clone(),
							x.6.clone(),
							x.7.clone(),
							x.8.clone(),
						),
					)
				})
				.collect::<Vec<_>>(),
		},
		babe: rococo_runtime::BabeConfig {
			authorities: Default::default(),
			epoch_config: Some(rococo_runtime::BABE_GENESIS_EPOCH_CONFIG),
		},
		grandpa: Default::default(),
		im_online: Default::default(),
		collective: Default::default(),
		membership: Default::default(),
		authority_discovery: rococo_runtime::AuthorityDiscoveryConfig { keys: vec![] },
		sudo: rococo_runtime::SudoConfig { key: Some(root_key.clone()) },
		hrmp: Default::default(),
		configuration: rococo_runtime::ConfigurationConfig {
			config: polkadot_runtime_parachains::configuration::HostConfiguration {
				max_validators_per_core: Some(1),
				..default_parachains_host_configuration()
			},
		},
		paras: rococo_runtime::ParasConfig { paras: vec![] },
		registrar: rococo_runtime::RegistrarConfig {
			next_free_para_id: polkadot_primitives::v2::LOWEST_PUBLIC_ID,
		},
		xcm_pallet: Default::default(),
		transaction_payment: Default::default(),
		bridge_rococo_grandpa: rococo_runtime::BridgeRococoGrandpaConfig {
			owner: Some(root_key.clone()),
			..Default::default()
		},
		bridge_wococo_grandpa: rococo_runtime::BridgeWococoGrandpaConfig {
			owner: Some(root_key.clone()),
			..Default::default()
		},
		bridge_rococo_messages: rococo_runtime::BridgeRococoMessagesConfig {
			owner: Some(root_key.clone()),
			..Default::default()
		},
		bridge_wococo_messages: rococo_runtime::BridgeWococoMessagesConfig {
			owner: Some(root_key.clone()),
			..Default::default()
		},
	}
}

#[cfg(feature = "polkadot-native")]
fn polkadot_development_config_genesis(wasm_binary: &[u8]) -> polkadot::GenesisConfig {
	polkadot_testnet_genesis(
		wasm_binary,
		vec![get_authority_keys_from_seed_no_beefy("Alice")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

#[cfg(feature = "kusama-native")]
fn kusama_development_config_genesis(wasm_binary: &[u8]) -> kusama::GenesisConfig {
	kusama_testnet_genesis(
		wasm_binary,
		vec![get_authority_keys_from_seed_no_beefy("Alice")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

#[cfg(feature = "westend-native")]
fn westend_development_config_genesis(wasm_binary: &[u8]) -> westend::GenesisConfig {
	westend_testnet_genesis(
		wasm_binary,
		vec![get_authority_keys_from_seed_no_beefy("Alice")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

#[cfg(feature = "rococo-native")]
fn rococo_development_config_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	rococo_testnet_genesis(
		wasm_binary,
		vec![get_authority_keys_from_seed("Alice")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Polkadot development config (single validator Alice)
#[cfg(feature = "polkadot-native")]
pub fn polkadot_development_config() -> Result<PolkadotChainSpec, String> {
	let wasm_binary = polkadot::WASM_BINARY.ok_or("Polkadot development wasm not available")?;

	Ok(PolkadotChainSpec::from_genesis(
		"Development",
		"dev",
		ChainType::Development,
		move || polkadot_development_config_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		Some(polkadot_chain_spec_properties()),
		Default::default(),
	))
}

/// Kusama development config (single validator Alice)
#[cfg(feature = "kusama-native")]
pub fn kusama_development_config() -> Result<KusamaChainSpec, String> {
	let wasm_binary = kusama::WASM_BINARY.ok_or("Kusama development wasm not available")?;

	Ok(KusamaChainSpec::from_genesis(
		"Development",
		"kusama_dev",
		ChainType::Development,
		move || kusama_development_config_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// Westend development config (single validator Alice)
#[cfg(feature = "westend-native")]
pub fn westend_development_config() -> Result<WestendChainSpec, String> {
	let wasm_binary = westend::WASM_BINARY.ok_or("Westend development wasm not available")?;

	Ok(WestendChainSpec::from_genesis(
		"Development",
		"westend_dev",
		ChainType::Development,
		move || westend_development_config_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// Rococo development config (single validator Alice)
#[cfg(feature = "rococo-native")]
pub fn rococo_development_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Rococo development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Development",
		"rococo_dev",
		ChainType::Development,
		move || RococoGenesisExt {
			runtime_genesis_config: rococo_development_config_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// `Versi` development config (single validator Alice)
#[cfg(feature = "rococo-native")]
pub fn versi_development_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Versi development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Development",
		"versi_dev",
		ChainType::Development,
		move || RococoGenesisExt {
			runtime_genesis_config: rococo_development_config_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some("versi"),
		None,
		None,
		Default::default(),
	))
}

/// Wococo development config (single validator Alice)
#[cfg(feature = "rococo-native")]
pub fn wococo_development_config() -> Result<RococoChainSpec, String> {
	const WOCOCO_DEV_PROTOCOL_ID: &str = "woco";
	let wasm_binary = rococo::WASM_BINARY.ok_or("Wococo development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Development",
		"wococo_dev",
		ChainType::Development,
		move || RococoGenesisExt {
			runtime_genesis_config: rococo_development_config_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some(WOCOCO_DEV_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

#[cfg(feature = "polkadot-native")]
fn polkadot_local_testnet_genesis(wasm_binary: &[u8]) -> polkadot::GenesisConfig {
	polkadot_testnet_genesis(
		wasm_binary,
		vec![
			get_authority_keys_from_seed_no_beefy("Alice"),
			get_authority_keys_from_seed_no_beefy("Bob"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Polkadot local testnet config (multivalidator Alice + Bob)
#[cfg(feature = "polkadot-native")]
pub fn polkadot_local_testnet_config() -> Result<PolkadotChainSpec, String> {
	let wasm_binary = polkadot::WASM_BINARY.ok_or("Polkadot development wasm not available")?;

	Ok(PolkadotChainSpec::from_genesis(
		"Local Testnet",
		"local_testnet",
		ChainType::Local,
		move || polkadot_local_testnet_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		Some(polkadot_chain_spec_properties()),
		Default::default(),
	))
}

#[cfg(feature = "kusama-native")]
fn kusama_local_testnet_genesis(wasm_binary: &[u8]) -> kusama::GenesisConfig {
	kusama_testnet_genesis(
		wasm_binary,
		vec![
			get_authority_keys_from_seed_no_beefy("Alice"),
			get_authority_keys_from_seed_no_beefy("Bob"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Kusama local testnet config (multivalidator Alice + Bob)
#[cfg(feature = "kusama-native")]
pub fn kusama_local_testnet_config() -> Result<KusamaChainSpec, String> {
	let wasm_binary = kusama::WASM_BINARY.ok_or("Kusama development wasm not available")?;

	Ok(KusamaChainSpec::from_genesis(
		"Kusama Local Testnet",
		"kusama_local_testnet",
		ChainType::Local,
		move || kusama_local_testnet_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

#[cfg(feature = "westend-native")]
fn westend_local_testnet_genesis(wasm_binary: &[u8]) -> westend::GenesisConfig {
	westend_testnet_genesis(
		wasm_binary,
		vec![
			get_authority_keys_from_seed_no_beefy("Alice"),
			get_authority_keys_from_seed_no_beefy("Bob"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Westend local testnet config (multivalidator Alice + Bob)
#[cfg(feature = "westend-native")]
pub fn westend_local_testnet_config() -> Result<WestendChainSpec, String> {
	let wasm_binary = westend::WASM_BINARY.ok_or("Westend development wasm not available")?;

	Ok(WestendChainSpec::from_genesis(
		"Westend Local Testnet",
		"westend_local_testnet",
		ChainType::Local,
		move || westend_local_testnet_genesis(wasm_binary),
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

#[cfg(feature = "rococo-native")]
fn rococo_local_testnet_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	rococo_testnet_genesis(
		wasm_binary,
		vec![get_authority_keys_from_seed("Alice"), get_authority_keys_from_seed("Bob")],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Rococo local testnet config (multivalidator Alice + Bob)
#[cfg(feature = "rococo-native")]
pub fn rococo_local_testnet_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Rococo development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Rococo Local Testnet",
		"rococo_local_testnet",
		ChainType::Local,
		move || RococoGenesisExt {
			runtime_genesis_config: rococo_local_testnet_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// Wococo is a temporary testnet that uses almost the same runtime as rococo.
#[cfg(feature = "rococo-native")]
fn wococo_local_testnet_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	rococo_testnet_genesis(
		wasm_binary,
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
			get_authority_keys_from_seed("Charlie"),
			get_authority_keys_from_seed("Dave"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// Wococo local testnet config (multivalidator Alice + Bob + Charlie + Dave)
#[cfg(feature = "rococo-native")]
pub fn wococo_local_testnet_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Wococo development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Wococo Local Testnet",
		"wococo_local_testnet",
		ChainType::Local,
		move || RococoGenesisExt {
			runtime_genesis_config: wococo_local_testnet_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some(DEFAULT_PROTOCOL_ID),
		None,
		None,
		Default::default(),
	))
}

/// `Versi` is a temporary testnet that uses the same runtime as rococo.
#[cfg(feature = "rococo-native")]
fn versi_local_testnet_genesis(wasm_binary: &[u8]) -> rococo_runtime::GenesisConfig {
	rococo_testnet_genesis(
		wasm_binary,
		vec![
			get_authority_keys_from_seed("Alice"),
			get_authority_keys_from_seed("Bob"),
			get_authority_keys_from_seed("Charlie"),
			get_authority_keys_from_seed("Dave"),
		],
		get_account_id_from_seed::<sr25519::Public>("Alice"),
		None,
	)
}

/// `Versi` local testnet config (multivalidator Alice + Bob + Charlie + Dave)
#[cfg(feature = "rococo-native")]
pub fn versi_local_testnet_config() -> Result<RococoChainSpec, String> {
	let wasm_binary = rococo::WASM_BINARY.ok_or("Versi development wasm not available")?;

	Ok(RococoChainSpec::from_genesis(
		"Versi Local Testnet",
		"versi_local_testnet",
		ChainType::Local,
		move || RococoGenesisExt {
			runtime_genesis_config: versi_local_testnet_genesis(wasm_binary),
			// Use 1 minute session length.
			session_length_in_blocks: Some(10),
		},
		vec![],
		None,
		Some("versi"),
		None,
		None,
		Default::default(),
	))
}
