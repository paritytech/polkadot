// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A list of the different weight modules for our runtime.

pub mod frame_system;
pub mod pallet_balances;
pub mod pallet_balances_nis_counterpart_balances;
pub mod pallet_bounties;
pub mod pallet_child_bounties;
pub mod pallet_collective_council;
pub mod pallet_collective_technical_committee;
pub mod pallet_democracy;
pub mod pallet_elections_phragmen;
pub mod pallet_identity;
pub mod pallet_im_online;
pub mod pallet_indices;
pub mod pallet_membership;
pub mod pallet_message_queue;
pub mod pallet_multisig;
pub mod pallet_nis;
pub mod pallet_preimage;
pub mod pallet_proxy;
pub mod pallet_scheduler;
pub mod pallet_session;
pub mod pallet_sudo;
pub mod pallet_timestamp;
pub mod pallet_tips;
pub mod pallet_treasury;
pub mod pallet_utility;
pub mod pallet_vesting;
pub mod pallet_xcm;
pub mod runtime_common_assigned_slots;
pub mod runtime_common_auctions;
pub mod runtime_common_claims;
pub mod runtime_common_crowdloan;
pub mod runtime_common_paras_registrar;
pub mod runtime_common_slots;
pub mod runtime_parachains_assigner_on_demand;
pub mod runtime_parachains_configuration;
pub mod runtime_parachains_disputes;
pub mod runtime_parachains_hrmp;
pub mod runtime_parachains_inclusion;
pub mod runtime_parachains_initializer;
pub mod runtime_parachains_paras;
pub mod runtime_parachains_paras_inherent;
pub mod xcm;
