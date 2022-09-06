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

use super::*;

pub(crate) fn impl_orchestra_struct(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let message_wrapper = &info.message_wrapper.clone();
	let orchestra_name = info.orchestra_name.clone();
	let subsystem_name = &info.subsystem_names_without_wip();
	let support_crate = info.support_crate_name();

	let baggage_decl = &info.baggage_decl();

	let baggage_generic_ty = &info.baggage_generic_types();

	let generics = quote! {
		< S, #( #baggage_generic_ty, )* >
	};

	let where_clause = quote! {
		where
			S: #support_crate ::Spawner,
	};
	// TODO add `where ..` clauses for baggage types
	// TODO <https://github.com/paritytech/polkadot/issues/3427>

	let consumes = &info.consumes_without_wip();
	let consumes_variant = &info.variant_names_without_wip();
	let unconsumes_variant = &info.variant_names_only_wip();

	let signal_ty = &info.extern_signal_ty;

	let error_ty = &info.extern_error_ty;

	let event_ty = &info.extern_event_ty;

	let message_channel_capacity = info.message_channel_capacity;
	let signal_channel_capacity = info.signal_channel_capacity;

	let log_target =
		syn::LitStr::new(orchestra_name.to_string().to_lowercase().as_str(), orchestra_name.span());

	let ts = quote! {
		/// Capacity of a bounded message channel between orchestra and subsystem
		/// but also for bounded channels between two subsystems.
		const CHANNEL_CAPACITY: usize = #message_channel_capacity;

		/// Capacity of a signal channel between a subsystem and the orchestra.
		const SIGNAL_CHANNEL_CAPACITY: usize = #signal_channel_capacity;

		/// The log target tag.
		const LOG_TARGET: &'static str = #log_target;

		/// The orchestra.
		pub struct #orchestra_name #generics {

			#(
				/// A subsystem instance.
				#subsystem_name: OrchestratedSubsystem< #consumes >,
			)*

			#(
				/// A user specified addendum field.
				#baggage_decl ,
			)*

			/// Responsible for driving the subsystem futures.
			spawner: S,

			/// The set of running subsystems.
			running_subsystems: #support_crate ::FuturesUnordered<
				BoxFuture<'static, ::std::result::Result<(), #error_ty>>
			>,

			/// Gather running subsystems' outbound streams into one.
			to_orchestra_rx: #support_crate ::stream::Fuse<
				#support_crate ::metered::UnboundedMeteredReceiver< #support_crate ::ToOrchestra >
			>,

			/// Events that are sent to the orchestra from the outside world.
			events_rx: #support_crate ::metered::MeteredReceiver< #event_ty >,
		}

		impl #generics #orchestra_name #generics #where_clause {
			/// Send the given signal, a termination signal, to all subsystems
			/// and wait for all subsystems to go down.
			///
			/// The definition of a termination signal is up to the user and
			/// implementation specific.
			pub async fn wait_terminate(&mut self, signal: #signal_ty, timeout: ::std::time::Duration) -> ::std::result::Result<(), #error_ty > {
				#(
					::std::mem::drop(self. #subsystem_name .send_signal(signal.clone()).await);
				)*
				let _ = signal;

				let mut timeout_fut = #support_crate ::Delay::new(
						timeout
					).fuse();

				loop {
					#support_crate ::futures::select! {
						_ = self.running_subsystems.next() =>
						if self.running_subsystems.is_empty() {
							break;
						},
						_ = timeout_fut => break,
						complete => break,
					}
				}

				Ok(())
			}

			/// Broadcast a signal to all subsystems.
			pub async fn broadcast_signal(&mut self, signal: #signal_ty) -> ::std::result::Result<(), #error_ty > {
				#(
					let _ = self. #subsystem_name .send_signal(signal.clone()).await;
				)*
				let _ = signal;

				Ok(())
			}

			/// Route a particular message to a subsystem that consumes the message.
			pub async fn route_message(&mut self, message: #message_wrapper, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				match message {
					#(
						#message_wrapper :: #consumes_variant ( inner ) =>
							OrchestratedSubsystem::< #consumes >::send_message2(&mut self. #subsystem_name, inner, origin ).await?,
					)*
					// subsystems that are still work in progress
					#(
						#message_wrapper :: #unconsumes_variant ( _ ) => {}
					)*
					#message_wrapper :: Empty => {}

					// And everything that's not WIP but no subsystem consumes it
					#[allow(unreachable_patterns)]
					unused_msg => {
						#support_crate :: tracing :: warn!("Nothing consumes {:?}", unused_msg);
					}
				}
				Ok(())
			}

			/// Extract information from each subsystem.
			pub fn map_subsystems<'a, Mapper, Output>(&'a self, mapper: Mapper)
			-> Vec<Output>
				where
				#(
					Mapper: MapSubsystem<&'a OrchestratedSubsystem< #consumes >, Output=Output>,
				)*
			{
				vec![
				#(
					mapper.map_subsystem( & self. #subsystem_name ),
				)*
				]
			}

			/// Get access to internal task spawner.
			pub fn spawner<'a> (&'a mut self) -> &'a mut S {
				&mut self.spawner
			}
		}

	};

	ts
}

pub(crate) fn impl_orchestrated_subsystem(info: &OrchestraInfo) -> proc_macro2::TokenStream {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let ts = quote::quote! {
		/// A subsystem that the orchestrator orchestrates.
		///
		/// Ties together the [`Subsystem`] itself and it's running instance
		/// (which may be missing if the [`Subsystem`] is not running at the moment
		/// for whatever reason).
		///
		/// [`Subsystem`]: trait.Subsystem.html
		pub struct OrchestratedSubsystem<M> {
			/// The instance.
			pub instance: std::option::Option<
				#support_crate ::SubsystemInstance<M, #signal>
			>,
		}

		impl<M> OrchestratedSubsystem<M> {
			/// Send a message to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_message2(&mut self, message: M, origin: &'static str) -> ::std::result::Result<(), #error_ty > {
				const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_bounded.send(MessagePacket {
						signals_received: instance.signals_received,
						message: message.into(),
					}).timeout(MESSAGE_TIMEOUT).await
					{
						None => {
							#support_crate ::tracing::error!(
								target: LOG_TARGET,
								%origin,
								"Subsystem {} appears unresponsive.",
								instance.name,
							);
							Err(#error_ty :: from(
								#support_crate ::OrchestraError::SubsystemStalled(instance.name)
							))
						}
						Some(res) => res.map_err(Into::into),
					}
				} else {
					Ok(())
				}
			}

			/// Send a signal to the wrapped subsystem.
			///
			/// If the inner `instance` is `None`, nothing is happening.
			pub async fn send_signal(&mut self, signal: #signal) -> ::std::result::Result<(), #error_ty > {
				const SIGNAL_TIMEOUT: ::std::time::Duration = ::std::time::Duration::from_secs(10);

				if let Some(ref mut instance) = self.instance {
					match instance.tx_signal.send(signal).timeout(SIGNAL_TIMEOUT).await {
						None => {
							Err(#error_ty :: from(
								#support_crate ::OrchestraError::SubsystemStalled(instance.name)
							))
						}
						Some(res) => {
							let res = res.map_err(Into::into);
							if res.is_ok() {
								instance.signals_received += 1;
							}
							res
						}
					}
				} else {
					Ok(())
				}
			}
		}
	};
	ts
}
