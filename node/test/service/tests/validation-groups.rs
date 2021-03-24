// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use polkadot_test_service::{new_full, node_config};
use futures::future::Future;
use polkadot_overseer::OverseerHandler;
use polkadot_primitives::v1::{
	Id as ParaId, HeadData, ValidationCode, Balance, CollatorPair, CollatorId,
};
use polkadot_runtime_common::BlockHashCount;
use polkadot_service::{
	Error, NewFull, FullClient, ClientHandle, ExecuteWithClient, IsCollator,
};
use polkadot_node_subsystem::messages::{CollatorProtocolMessage, CollationGenerationMessage};
use polkadot_test_runtime::{
	Runtime, SignedExtra, SignedPayload, VERSION, ParasSudoWrapperCall, SudoCall, UncheckedExtrinsic,
};
use polkadot_node_primitives::{CollatorFn, CollationGenerationConfig};
use polkadot_runtime_parachains::paras::ParaGenesisArgs;
use sc_chain_spec::ChainSpec;
use sc_client_api::execution_extensions::ExecutionStrategies;
use sc_executor::native_executor_instance;
use sc_network::{
	config::{NetworkConfiguration, TransportConfig},
	multiaddr,
};
use service::{
	config::{DatabaseConfig, KeystoreConfig, MultiaddrWithPeerId, WasmExecutionMethod},
	RpcHandlers, TaskExecutor, TaskManager, KeepBlocks, TransactionStorageMode,
};
use service::{BasePath, Configuration, Role};
use sp_arithmetic::traits::SaturatedConversion;
use sp_blockchain::HeaderBackend;
use sp_keyring::Sr25519Keyring;
use sp_runtime::{codec::Encode, generic, traits::IdentifyAccount, MultiSigner};
use sp_state_machine::BasicExternalities;
use std::sync::Arc;
use substrate_test_client::{BlockchainEventsExt, RpcHandlersExt, RpcTransactionOutput, RpcTransactionError};

use polkadot_test_service::{PolkadotTestNode, run_validator_node};
use futures::{future, pin_mut, select};

pub(crate) struct KeysIter(Vec<Sr25519Keyring>);

impl KeysIter {
	pub(crate) fn new() -> Self {
		KeysIter(vec![
			// grp 1
			Sr25519Keyring::Alice,
			Sr25519Keyring::Bob,
			// grp 2
			Sr25519Keyring::Charlie,
			Sr25519Keyring::Dave,
			// grp 3
			Sr25519Keyring::Eve,
			Sr25519Keyring::Ferdie,
		])
	}
}

impl Iterator for KeysIter {
	type Item = Sr25519Keyring;
	fn next(&mut self) -> Option<Self::Item> {
		self.0.pop()
	}
}


#[substrate_test_utils::test(core_threads = 8)]
async fn multiple_validator_groups_work(task_executor: TaskExecutor) {
	let mut builder = sc_cli::LoggerBuilder::new("");
	builder.with_colors(false);
	builder.init().expect("Setting up logger always works. qed");

	// create nodes with all previous nodes as boot nodes
	let nodes = KeysIter::new()
		.scan(Vec::<MultiaddrWithPeerId>::new(), |boot_nodes, key: Sr25519Keyring| {
			let node = run_validator_node(
				task_executor.clone(),
				key,
				|| {},
				boot_nodes.clone(),
			);

			boot_nodes.push(node.addr.clone());

			Some(node)
		})
		.collect::<Vec<PolkadotTestNode>>();

	{
		let fut = future::join_all(nodes.iter().map(|node| {
			node.wait_for_blocks(10).fuse()
		}) ).fuse();

		pin_mut!(fut);

		fut.await;
	}

	for node in nodes {
		node.task_manager.clean_shutdown().await;
	}
}
