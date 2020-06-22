// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Client testing utilities.

#![warn(missing_docs)]

use std::sync::Arc;
use std::collections::BTreeMap;
use std::convert::TryFrom;
pub use substrate_test_client::*;
pub use polkadot_test_runtime as runtime;

use sp_core::{sr25519, ChangesTrieConfiguration, map, twox_128};
use sp_core::storage::{ChildInfo, Storage, StorageChild};
use polkadot_test_runtime::genesismap::GenesisConfig;
use sp_runtime::traits::{Block as BlockT, Header as HeaderT, Hash as HashT, HashFor};
use sc_consensus::LongestChain;
use sc_client_api::light::{RemoteCallRequest, RemoteBodyRequest};
use sc_service::client::{
	genesis, Client as SubstrateClient, LocalCallExecutor
};
use sc_light::{
	call_executor::GenesisCallExecutor, backend as light_backend,
	new_light_blockchain, new_light_backend,
};

/// A prelude to import in tests.
pub mod prelude {
	// Trait extensions
	pub use super::{ClientExt, ClientBlockImportExt};
	// Client structs
	pub use super::{
		TestClient, TestClientBuilder, Backend, LightBackend,
		Executor, LightExecutor, LocalExecutor, NativeExecutor, WasmExecutionMethod,
	};
	// Keyring
	pub use super::{AccountKeyring, Sr25519Keyring};
}

sc_executor::native_executor_instance! {
	pub LocalExecutor,
	polkadot_test_runtime::api::dispatch,
	polkadot_test_runtime::native_version,
}

/// Test client database backend.
pub type Backend = substrate_test_client::Backend<polkadot_test_runtime::Block>;

/// Test client executor.
pub type Executor = LocalCallExecutor<
	Backend,
	NativeExecutor<LocalExecutor>,
>;

/// Test client light database backend.
pub type LightBackend = substrate_test_client::LightBackend<polkadot_test_runtime::Block>;

/// Test client light executor.
pub type LightExecutor = GenesisCallExecutor<
	LightBackend,
	LocalCallExecutor<
		light_backend::Backend<
			sc_client_db::light::LightStorage<polkadot_test_runtime::Block>,
			HashFor<polkadot_test_runtime::Block>
		>,
		NativeExecutor<LocalExecutor>
	>
>;

/// Parameters of test-client builder with test-runtime.
#[derive(Default)]
pub struct GenesisParameters {
	changes_trie_config: Option<ChangesTrieConfiguration>,
	extra_storage: Storage,
}

impl GenesisParameters {
	fn genesis_config(&self) -> GenesisConfig {
		GenesisConfig::new(
			self.changes_trie_config.clone(),
			vec![
				sr25519::Public::from(Sr25519Keyring::Alice).into(),
				sr25519::Public::from(Sr25519Keyring::Bob).into(),
				sr25519::Public::from(Sr25519Keyring::Charlie).into(),
			],
			1000,
			self.extra_storage.clone(),
		)
	}
}

fn additional_storage_with_genesis(genesis_block: &polkadot_test_runtime::Block) -> BTreeMap<Vec<u8>, Vec<u8>> {
	map![
		twox_128(&b"latest"[..]).to_vec() => genesis_block.hash().as_fixed_bytes().to_vec()
	]
}

impl substrate_test_client::GenesisInit for GenesisParameters {
	fn genesis_storage(&self) -> Storage {
		use codec::Encode;

		let mut storage = self.genesis_config().genesis_map();

		let child_roots = storage.children_default.iter().map(|(sk, child_content)| {
			let state_root = <<<runtime::Block as BlockT>::Header as HeaderT>::Hashing as HashT>::trie_root(
				child_content.data.clone().into_iter().collect()
			);
			(sk.clone(), state_root.encode())
		});
		let state_root = <<<runtime::Block as BlockT>::Header as HeaderT>::Hashing as HashT>::trie_root(
			storage.top.clone().into_iter().chain(child_roots).collect()
		);
		let block: runtime::Block = genesis::construct_genesis_block(state_root);
		storage.top.extend(additional_storage_with_genesis(&block));

		storage
	}
}

/// A `TestClient` with `test-runtime` builder.
pub type TestClientBuilder<E, B> = substrate_test_client::TestClientBuilder<
	polkadot_test_runtime::Block,
	E,
	B,
	GenesisParameters,
>;

/// Test client type with `LocalExecutor` and generic Backend.
pub type Client<B> = SubstrateClient<
	B,
	LocalCallExecutor<B, sc_executor::NativeExecutor<LocalExecutor>>,
	polkadot_test_runtime::Block,
	polkadot_test_runtime::RuntimeApi,
>;

/// A test client with default backend.
pub type TestClient = Client<Backend>;

/// A `TestClientBuilder` with default backend and executor.
pub trait DefaultTestClientBuilderExt: Sized {
	/// Create new `TestClientBuilder`
	fn new() -> Self;
}

impl DefaultTestClientBuilderExt for TestClientBuilder<Executor, Backend> {
	fn new() -> Self {
		Self::with_default_backend()
	}
}

/// A `test-runtime` extensions to `TestClientBuilder`.
pub trait TestClientBuilderExt<B>: Sized {
	/// Returns a mutable reference to the genesis parameters.
	fn genesis_init_mut(&mut self) -> &mut GenesisParameters;

	/// Set changes trie configuration for genesis.
	fn changes_trie_config(mut self, config: Option<ChangesTrieConfiguration>) -> Self {
		self.genesis_init_mut().changes_trie_config = config;
		self
	}

	/// Add an extra value into the genesis storage.
	///
	/// # Panics
	///
	/// Panics if the key is empty.
	fn add_extra_child_storage<SK: Into<Vec<u8>>, K: Into<Vec<u8>>, V: Into<Vec<u8>>>(
		mut self,
		storage_key: SK,
		child_info: ChildInfo,
		key: K,
		value: V,
	) -> Self {
		let storage_key = storage_key.into();
		let key = key.into();
		assert!(!storage_key.is_empty());
		assert!(!key.is_empty());
		self.genesis_init_mut().extra_storage.children_default
			.entry(storage_key)
			.or_insert_with(|| StorageChild {
				data: Default::default(),
				child_info: child_info.to_owned(),
			}).data.insert(key, value.into());
		self
	}

	/// Add an extra child value into the genesis storage.
	///
	/// # Panics
	///
	/// Panics if the key is empty.
	fn add_extra_storage<K: Into<Vec<u8>>, V: Into<Vec<u8>>>(mut self, key: K, value: V) -> Self {
		let key = key.into();
		assert!(!key.is_empty());
		self.genesis_init_mut().extra_storage.top.insert(key, value.into());
		self
	}

	/// Build the test client.
	fn build(self) -> Client<B> {
		self.build_with_longest_chain().0
	}

	/// Build the test client and longest chain selector.
	fn build_with_longest_chain(self) -> (Client<B>, LongestChain<B, polkadot_test_runtime::Block>);

	/// Build the test client and the backend.
	fn build_with_backend(self) -> (Client<B>, Arc<B>);
}

impl TestClientBuilderExt<Backend> for TestClientBuilder<
	LocalCallExecutor<Backend, sc_executor::NativeExecutor<LocalExecutor>>,
	Backend
> {
	fn genesis_init_mut(&mut self) -> &mut GenesisParameters {
		Self::genesis_init_mut(self)
	}

	fn build_with_longest_chain(self) -> (Client<Backend>, LongestChain<Backend, polkadot_test_runtime::Block>) {
		self.build_with_native_executor(None)
	}

	fn build_with_backend(self) -> (Client<Backend>, Arc<Backend>) {
		let backend = self.backend();
		(self.build_with_native_executor(None).0, backend)
	}
}

/// Type of optional fetch callback.
type MaybeFetcherCallback<Req, Resp> = Option<Box<dyn Fn(Req) -> Result<Resp, sp_blockchain::Error> + Send + Sync>>;

/// Implementation of light client fetcher used in tests.
#[derive(Default)]
pub struct LightFetcher {
	call: MaybeFetcherCallback<RemoteCallRequest<polkadot_test_runtime::Header>, Vec<u8>>,
	body: MaybeFetcherCallback<RemoteBodyRequest<polkadot_test_runtime::Header>, Vec<polkadot_test_runtime::Extrinsic>>,
}

impl LightFetcher {
	/// Sets remote call callback.
	pub fn with_remote_call(
		self,
		call: MaybeFetcherCallback<RemoteCallRequest<polkadot_test_runtime::Header>, Vec<u8>>,
	) -> Self {
		LightFetcher {
			call,
			body: self.body,
		}
	}

	/// Sets remote body callback.
	pub fn with_remote_body(
		self,
		body: MaybeFetcherCallback<RemoteBodyRequest<polkadot_test_runtime::Header>, Vec<polkadot_test_runtime::Extrinsic>>,
	) -> Self {
		LightFetcher {
			call: self.call,
			body,
		}
	}
}

/// Creates new client instance used for tests.
pub fn new() -> Client<Backend> {
	TestClientBuilder::new().build()
}

/// Creates new light client instance used for tests.
pub fn new_light() -> (
	SubstrateClient<
		LightBackend,
		LightExecutor,
		polkadot_test_runtime::Block,
		polkadot_test_runtime::RuntimeApi
	>,
	Arc<LightBackend>,
) {

	let storage = sc_client_db::light::LightStorage::new_test();
	let blockchain =new_light_blockchain(storage);
	let backend = new_light_backend(blockchain.clone());
	let executor = new_native_executor();
	let local_call_executor = LocalCallExecutor::new(
		backend.clone(),
		executor,
		sp_core::tasks::executor(),
		Default::default()
	);
	let call_executor = LightExecutor::new(
		backend.clone(),
		local_call_executor,
	);

	(
		TestClientBuilder::with_backend(backend.clone())
			.build_with_executor(call_executor)
			.0,
		backend,
	)
}

/// Creates new light client fetcher used for tests.
pub fn new_light_fetcher() -> LightFetcher {
	LightFetcher::default()
}

/// Create a new native executor.
pub fn new_native_executor() -> sc_executor::NativeExecutor<LocalExecutor> {
	sc_executor::NativeExecutor::new(sc_executor::WasmExecutionMethod::Interpreted, None, 8)
}

/// Extrinsics that must be included in each block.
///
/// The index of the block must be provided to calculate a valid timestamp for the block. The value starts at 0 and
/// should be incremented by one for every block produced.
pub fn needed_extrinsics(
	heads: Vec<polkadot_primitives::parachain::AttestedCandidate>,
	i: u64,
) -> Vec<polkadot_test_runtime::UncheckedExtrinsic> {
	use polkadot_runtime_common::parachains;

	let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
		.expect("now always later than unix epoch; qed")
		.as_millis() + (i * polkadot_test_runtime::constants::time::SLOT_DURATION / 2) as u128;

	vec![
		polkadot_test_runtime::UncheckedExtrinsic {
			function: polkadot_test_runtime::Call::Parachains(parachains::Call::set_heads(heads)),
			signature: None,
		},
		polkadot_test_runtime::UncheckedExtrinsic {
			function: polkadot_test_runtime::Call::Timestamp(pallet_timestamp::Call::set(
				u64::try_from(timestamp).expect("unexpectedly big timestamp"),
			)),
			signature: None,
		}
	]
}
