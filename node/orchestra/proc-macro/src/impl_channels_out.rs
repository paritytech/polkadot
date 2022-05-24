// Copyright (C) 2021 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use quote::quote;
use syn::Result;

use super::*;

/// Implement the helper type `ChannelsOut` and `MessagePacket<T>`.
pub(crate) fn impl_channels_out_struct(info: &OrchestraInfo) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = info.message_wrapper.clone();

	let channel_name = &info.channel_names_without_wip("");
	let channel_name_unbounded = &info.channel_names_without_wip("_unbounded");

	let consumes = &info.consumes_without_wip();

	let consumes_variant = &info.variant_names_without_wip();
	let unconsumes_variant = &info.variant_names_only_wip();

	let support_crate = info.support_crate_name();

	let ts = quote! {
		/// Collection of channels to the individual subsystems.
		///
		/// Naming is from the point of view of the orchestra.
		#[derive(Debug, Clone)]
		pub struct ChannelsOut {
			#(
				/// Bounded channel sender, connected to a subsystem.
				pub #channel_name:
					#support_crate ::metered::MeteredSender<
						MessagePacket< #consumes >
					>,
			)*

			#(
				/// Unbounded channel sender, connected to a subsystem.
				pub #channel_name_unbounded:
					#support_crate ::metered::UnboundedMeteredSender<
						MessagePacket< #consumes >
					>,
			)*
		}

		#[allow(unreachable_code)]
		// when no defined messages in enum
		impl ChannelsOut {
			/// Send a message via a bounded channel.
			pub async fn send_and_log_error(
				&mut self,
				signals_received: usize,
				message: #message_wrapper,
			) {

				let res: ::std::result::Result<_, _> = match message {
				#(
					#message_wrapper :: #consumes_variant ( inner ) => {
						self. #channel_name .send(
							#support_crate ::make_packet(signals_received, inner)
						).await.map_err(|_| stringify!( #channel_name ))
					}
				)*
					// subsystems that are wip
				#(
					#message_wrapper :: #unconsumes_variant ( _ ) => Ok(()),
				)*
					// dummy message type
					#message_wrapper :: Empty => Ok(()),

					#[allow(unreachable_patterns)]
					// And everything that's not WIP but no subsystem consumes it
					unused_msg => {
						#support_crate :: tracing :: warn!("Nothing consumes {:?}", unused_msg);
						Ok(())
					}
				};

				if let Err(subsystem_name) = res {
					#support_crate ::tracing::debug!(
						target: LOG_TARGET,
						"Failed to send (bounded) a message to {} subsystem",
						subsystem_name
					);
				}
			}

			/// Send a message to another subsystem via an unbounded channel.
			pub fn send_unbounded_and_log_error(
				&self,
				signals_received: usize,
				message: #message_wrapper,
			) {
				let res: ::std::result::Result<_, _> = match message {
				#(
					#message_wrapper :: #consumes_variant (inner) => {
						self. #channel_name_unbounded .unbounded_send(
							#support_crate ::make_packet(signals_received, inner)
						)
						.map_err(|_| stringify!( #channel_name ))
					},
				)*
					// subsystems that are wip
				#(
					#message_wrapper :: #unconsumes_variant ( _ ) => Ok(()),
				)*
					// dummy message type
					#message_wrapper :: Empty => Ok(()),

					// And everything that's not WIP but no subsystem consumes it
					#[allow(unreachable_patterns)]
					unused_msg => {
						#support_crate :: tracing :: warn!("Nothing consumes {:?}", unused_msg);
						Ok(())
					}
				};

				if let Err(subsystem_name) = res {
					#support_crate ::tracing::debug!(
						target: LOG_TARGET,
						"Failed to send_unbounded a message to {} subsystem",
						subsystem_name
					);
				}
			}
		}

	};
	Ok(ts)
}
