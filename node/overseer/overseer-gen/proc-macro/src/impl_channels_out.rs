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

use quote::quote;
use syn::Result;

use super::*;

/// Implement the helper type `ChannelsOut` and `MessagePacket<T>`.
pub(crate) fn impl_channels_out_struct(info: &OverseerInfo) -> Result<proc_macro2::TokenStream> {
	let message_wrapper = info.message_wrapper.clone();

	let channel_name = &info.channel_names_without_wip("");
	let channel_name_unbounded = &info.channel_names_without_wip("_unbounded");

	let consumes = &info.consumes_without_wip();
	let unconsumes = &info.consumes_only_wip();

	let ts = quote! {
		/// Collection of channels to the individual subsystems.
		///
		/// Naming is from the point of view of the overseer.
		#[derive(Debug, Clone)]
		pub struct ChannelsOut {
			#(
				/// Bounded channel sender, connected to a subsystem.
				pub #channel_name:
					::polkadot_overseer_gen::metered::MeteredSender<
						MessagePacket< #consumes >
					>,
			)*

			#(
				/// Unbounded channel sender, connected to a subsystem.
				pub #channel_name_unbounded:
					::polkadot_overseer_gen::metered::UnboundedMeteredSender<
						MessagePacket< #consumes >
					>,
			)*
		}

		impl ChannelsOut {
			/// Send a message via a bounded channel.
			pub async fn send_and_log_error(
				&mut self,
				signals_received: usize,
				message: #message_wrapper,
			) {
				let res: ::std::result::Result<_, _> = match message {
				#(
					#message_wrapper :: #consumes ( inner ) => {
						self. #channel_name .send(
							::polkadot_overseer_gen::make_packet(signals_received, inner)
						).await
					}
				)*
				// subsystems that are wip
				#(
					#message_wrapper :: #unconsumes ( _ ) => Ok(()),
				)*
				};

				if res.is_err() {
					::polkadot_overseer_gen::tracing::debug!(
						target: LOG_TARGET,
						"Failed to send a message to another subsystem",
					);
				}
			}

			/// Send a message to another subsystem via an unbounded channel.
			pub fn send_unbounded_and_log_error(
				&self,
				signals_received: usize,
				message: #message_wrapper,
			) {
				use ::std::sync::mpsc::TrySendError;

				let res: ::std::result::Result<_, _> = match message {
				#(
					#message_wrapper :: #consumes (inner) => {
						self. #channel_name_unbounded .unbounded_send(
							::polkadot_overseer_gen::make_packet(signals_received, inner)
						)
						.map_err(|e| e.into_send_error())
					},
				)*
				// subsystems that are wip
				#(
					#message_wrapper :: #unconsumes ( _ ) => Ok(()),
				)*
				};

				if res.is_err() {
					::polkadot_overseer_gen::tracing::debug!(
						target: LOG_TARGET,
						"Failed to send a message to another subsystem",
					);
				}
			}
		}

	};
	Ok(ts)
}
