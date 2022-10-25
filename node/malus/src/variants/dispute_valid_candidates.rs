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

//! A malicious node that replaces approvals with invalid disputes
//! against valid candidates. Additionally, the malus node can be configured to
//! fake candidate validation and return a static result for candidate checking.
//!
//! Attention: For usage with `zombienet` only!

#![allow(missing_docs)]

use polkadot_cli::{
	prepared_overseer_builder,
	service::{
		AuthorityDiscoveryApi, AuxStore, BabeApi, Block, Error, HeaderBackend, Overseer,
		OverseerConnector, OverseerGen, OverseerGenArgs, OverseerHandle, ParachainHost,
		ProvideRuntimeApi,
	},
	Cli,
};
use polkadot_node_subsystem::SpawnGlue;
use sp_core::traits::SpawnNamed;

// Filter wrapping related types.
use super::common::{FakeCandidateValidation, FakeCandidateValidationError};
use crate::{interceptor::*, variants::ReplaceValidationResult};

use std::sync::Arc;

#[derive(Debug, clap::Parser)]
#[command(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub struct DisputeAncestorOptions {
	/// Malicious candidate validation subsystem configuration. When enabled, node PVF execution is skipped
	/// during backing and/or approval and it's result can by specified by this option and `--fake-validation-error`
	/// for invalid candidate outcomes.
	#[arg(long, value_enum, ignore_case = true, default_value_t = FakeCandidateValidation::BackingAndApprovalInvalid)]
	pub fake_validation: FakeCandidateValidation,

	/// Applies only when `--fake-validation` is configured to reject candidates as invalid. It allows
	/// to specify the exact error to return from the malicious candidate validation subsystem.
	#[arg(long, value_enum, ignore_case = true, default_value_t = FakeCandidateValidationError::InvalidOutputs)]
	pub fake_validation_error: FakeCandidateValidationError,

	/// Determines the percentage of candidates that should be disputed. Allows for fine-tuning
	/// the intensity of the behavior of the malicious node. Value must be in the range [0..=100].
	#[clap(short, long, ignore_case = true, default_value_t = 100, value_parser = clap::value_parser!(u8).range(0..=100))]
	pub percentage: u8,

	#[clap(flatten)]
	pub cli: Cli,
}

pub(crate) struct DisputeValidCandidates {
	/// Fake validation config (applies to disputes as well).
	pub fake_validation: FakeCandidateValidation,
	/// Fake validation error config.
	pub fake_validation_error: FakeCandidateValidationError,
	/// The probability of behaving maliciously.
	pub percentage: u8,
}

impl OverseerGen for DisputeValidCandidates {
	fn generate<'a, Spawner, RuntimeClient>(
		&self,
		connector: OverseerConnector,
		args: OverseerGenArgs<'a, Spawner, RuntimeClient>,
	) -> Result<(Overseer<SpawnGlue<Spawner>, Arc<RuntimeClient>>, OverseerHandle), Error>
	where
		RuntimeClient: 'static + ProvideRuntimeApi<Block> + HeaderBackend<Block> + AuxStore,
		RuntimeClient::Api: ParachainHost<Block> + BabeApi<Block> + AuthorityDiscoveryApi<Block>,
		Spawner: 'static + SpawnNamed + Clone + Unpin,
	{
		let spawner = args.spawner.clone();
		let validation_filter = ReplaceValidationResult::new(
			self.fake_validation,
			self.fake_validation_error,
			f64::from(self.percentage),
			SpawnGlue(spawner.clone()),
		);

		prepared_overseer_builder(args)?
			.replace_candidate_validation(move |cv_subsystem| {
				InterceptedSubsystem::new(cv_subsystem, validation_filter)
			})
			.build_with_connector(connector)
			.map_err(|e| e.into())
	}
}
