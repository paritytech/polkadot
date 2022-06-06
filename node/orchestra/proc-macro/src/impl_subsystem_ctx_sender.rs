// Copyright (C) 2022 Parity Technologies (UK) Ltd.
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

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Path, Result, Type};

use petgraph::{visit::EdgeRef, Direction};

use super::*;

/// Generates all subsystem types and related accumulation traits.
pub(crate) fn impl_subsystem_types_all(info: &OrchestraInfo) -> Result<TokenStream> {
	let mut ts = TokenStream::new();

	let orchestra_name = &info.orchestra_name;
	let span = orchestra_name.span();
	let all_messages_wrapper = &info.message_wrapper;
	let support_crate = info.support_crate_name();
	let signal_ty = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;

	let cg = graph::ConnectionGraph::construct(info.subsystems());
	let graph = &cg.graph;

	// All outgoing edges are now usable to derive everything we need
	for node_index in graph.node_indices() {
		let subsystem_name = graph[node_index].to_string();
		let outgoing_wrapper = Ident::new(&(subsystem_name + "OutgoingMessages"), span);

		// cannot be a hashmap, duplicate keys and sorting required
		// maps outgoing messages to the subsystem that consumes it
		let outgoing_to_consumer = graph
			.edges_directed(node_index, Direction::Outgoing)
			.map(|edge| {
				let message_ty = edge.weight();
				let subsystem_generic_consumer = graph[edge.target()].clone();
				Ok((to_variant(message_ty, span.clone())?, subsystem_generic_consumer))
			})
			.collect::<Result<Vec<(Ident, Ident)>>>()?;

		// Split it for usage with quote
		let outgoing_variant = outgoing_to_consumer.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
		let subsystem_generic = outgoing_to_consumer.into_iter().map(|x| x.1).collect::<Vec<_>>();

		ts.extend(quote! {
			impl ::std::convert::From< #outgoing_wrapper > for #all_messages_wrapper {
				fn from(message: #outgoing_wrapper) -> Self {
					match message {
					#(
						#outgoing_wrapper :: #outgoing_variant ( msg ) => #all_messages_wrapper :: #subsystem_generic ( msg ),
					)*
						#outgoing_wrapper :: Empty => #all_messages_wrapper :: Empty,
						// And everything that's not WIP but no subsystem consumes it
						#[allow(unreachable_patterns)]
						unused_msg => {
							#support_crate :: tracing :: warn!("Nothing consumes {:?}", unused_msg);
							#all_messages_wrapper :: Empty
						}
					}
				}
			}
		})
	}

	// Dump the graph to file.
	#[cfg(feature = "graph")]
	{
		let path = std::path::PathBuf::from(env!("OUT_DIR"))
			.join(orchestra_name.to_string().to_lowercase() + "-subsystem-messaging.dot");
		if let Err(e) = std::fs::OpenOptions::new()
			.truncate(true)
			.create(true)
			.write(true)
			.open(&path)
			.and_then(|mut f| cg.graphviz(&mut f))
		{
			eprintln!("Failed to write dot graph to {}: {:?}", path.display(), e);
		} else {
			println!("Wrote dot graph to {}", path.display());
		}
	}

	let subsystem_sender_name = &Ident::new(&(orchestra_name.to_string() + "Sender"), span);
	let subsystem_ctx_name = &Ident::new(&(orchestra_name.to_string() + "SubsystemContext"), span);
	ts.extend(impl_subsystem_context(info, &subsystem_sender_name, &subsystem_ctx_name));

	ts.extend(impl_associate_outgoing_messages_trait(&all_messages_wrapper));

	ts.extend(impl_subsystem_sender(
		support_crate,
		info.subsystems().iter().map(|ssf| {
			let outgoing_wrapper =
				Ident::new(&(ssf.generic.to_string() + "OutgoingMessages"), span);
			outgoing_wrapper
		}),
		&all_messages_wrapper,
		&subsystem_sender_name,
	));

	// Create all subsystem specific types, one by one
	for ssf in info.subsystems() {
		let subsystem_name = ssf.generic.to_string();
		let outgoing_wrapper = &Ident::new(&(subsystem_name.clone() + "OutgoingMessages"), span);

		let subsystem_ctx_trait = &Ident::new(&(subsystem_name.clone() + "ContextTrait"), span);
		let subsystem_sender_trait = &Ident::new(&(subsystem_name.clone() + "SenderTrait"), span);

		ts.extend(impl_per_subsystem_helper_traits(
			info,
			subsystem_ctx_name,
			subsystem_ctx_trait,
			subsystem_sender_name,
			subsystem_sender_trait,
			&ssf.message_to_consume,
			&ssf.messages_to_send,
			outgoing_wrapper,
		));

		ts.extend(impl_associate_outgoing_messages(&ssf.message_to_consume, &outgoing_wrapper));

		ts.extend(impl_wrapper_enum(&outgoing_wrapper, ssf.messages_to_send.as_slice())?);
	}

	// impl the emtpy tuple handling for tests
	let empty_tuple: Type = parse_quote! { () };
	ts.extend(impl_subsystem_context_trait_for(
		empty_tuple.clone(),
		&[],
		empty_tuple,
		all_messages_wrapper,
		subsystem_ctx_name,
		subsystem_sender_name,
		support_crate,
		signal_ty,
		error_ty,
	));

	Ok(ts)
}

/// Extract the final component of the message type path as used in the `#[subsystem(consumes: path::to::Foo)]` annotation.
fn to_variant(path: &Path, span: Span) -> Result<Ident> {
	let ident = path
		.segments
		.last()
		.ok_or_else(|| syn::Error::new(span, "Path is empty, but it must end with an identifier"))
		.map(|segment| segment.ident.clone())?;
	Ok(ident)
}

/// Converts the outgoing message types to variants.
///
/// Note: Commonly this is `${X}Message` becomes `${X}OutgoingMessages::${X}Message`
/// where for `AllMessages` it would be `AllMessages::${X}`.
fn to_variants(message_types: &[Path], span: Span) -> Result<Vec<Ident>> {
	let variants: Vec<_> =
		Result::from_iter(message_types.into_iter().map(|path| to_variant(path, span.clone())))?;
	Ok(variants)
}

/// Generates the wrapper type enum, no bells or whistles.
pub(crate) fn impl_wrapper_enum(wrapper: &Ident, message_types: &[Path]) -> Result<TokenStream> {
	// The message types are path based, each of them must finish with a type
	// and as such we do this upfront.
	let variants = to_variants(message_types, wrapper.span())?;

	let ts = quote! {
		#[allow(missing_docs)]
		#[derive(Debug)]
		pub enum #wrapper {
			#(
				#variants ( #message_types ),
			)*
			Empty,
		}

		#(
			impl ::std::convert::From< #message_types > for #wrapper {
				fn from(message: #message_types) -> Self {
					#wrapper :: #variants ( message )
				}
			}
		)*

		// Useful for unit and integration tests:
		impl ::std::convert::From< () > for #wrapper {
			fn from(_message: ()) -> Self {
				#wrapper :: Empty
			}
		}
	};
	Ok(ts)
}

/// Create the subsystem sender type and implements `trait SubsystemSender`
/// for the `#outgoing_wrappers: From<OutgoingMessage>` with the proper associated types.
pub(crate) fn impl_subsystem_sender(
	support_crate: &Path,
	outgoing_wrappers: impl IntoIterator<Item = Ident>,
	all_messages_wrapper: &Ident,
	subsystem_sender_name: &Ident,
) -> TokenStream {
	let mut ts = quote! {
		/// Connector to send messages towards all subsystems,
		/// while tracking the which signals where already received.
		#[derive(Debug)]
		pub struct #subsystem_sender_name < OutgoingWrapper > {
			/// Collection of channels to all subsystems.
			channels: ChannelsOut,
			/// Systemwide tick for which signals were received by all subsystems.
			signals_received: SignalsReceived,
			/// Keep that marker around.
			_phantom: ::core::marker::PhantomData< OutgoingWrapper >,
		}

		// can't derive due to `PhantomData` and `OutgoingWrapper` not being
		// bounded by `Clone`.
		impl<OutgoingWrapper> std::clone::Clone for #subsystem_sender_name < OutgoingWrapper > {
			fn clone(&self) -> Self {
				Self {
					channels: self.channels.clone(),
					signals_received: self.signals_received.clone(),
					_phantom: ::core::marker::PhantomData::default(),
				}
			}
		}
	};

	// Create the same for a wrapping enum:
	//
	// 1. subsystem specific `*OutgoingMessages`-type
	// 2. orchestra-global-`AllMessages`-type
	let wrapped = |outgoing_wrapper: &TokenStream| {
		quote! {
			#[#support_crate ::async_trait]
			impl<OutgoingMessage> SubsystemSender< OutgoingMessage > for #subsystem_sender_name < #outgoing_wrapper >
			where
				OutgoingMessage: Send + 'static,
				#outgoing_wrapper: ::std::convert::From<OutgoingMessage> + Send,
				#all_messages_wrapper: ::std::convert::From< #outgoing_wrapper > + Send,
			{
				async fn send_message(&mut self, msg: OutgoingMessage)
				{
					self.channels.send_and_log_error(
						self.signals_received.load(),
						<#all_messages_wrapper as ::std::convert::From<_>> ::from (
							<#outgoing_wrapper as ::std::convert::From<_>> :: from ( msg )
						)
					).await;
				}

				async fn send_messages<I>(&mut self, msgs: I)
				where
					I: IntoIterator<Item=OutgoingMessage> + Send,
					I::IntoIter: Iterator<Item=OutgoingMessage> + Send,
				{
					for msg in msgs {
						self.send_message( msg ).await;
					}
				}

				fn send_unbounded_message(&mut self, msg: OutgoingMessage)
				{
					self.channels.send_unbounded_and_log_error(
						self.signals_received.load(),
						<#all_messages_wrapper as ::std::convert::From<_>> ::from (
							<#outgoing_wrapper as ::std::convert::From<_>> :: from ( msg )
						)
					);
				}
			}
		}
	};

	for outgoing_wrapper in outgoing_wrappers {
		ts.extend(wrapped(&quote! {
			#outgoing_wrapper
		}));
	}

	ts.extend(wrapped(&quote! {
		()
	}));

	ts
}

/// Define the `trait AssociateOutgoing` and implement it for  `#all_messages_wrapper` and `()`.
pub(crate) fn impl_associate_outgoing_messages_trait(all_messages_wrapper: &Ident) -> TokenStream {
	quote! {
		/// Binds a generated type covering all declared outgoing messages,
		/// which implements `#generated_outgoing: From<M>` for all annotated types
		/// of a particular subsystem.
		///
		/// Note: This works because there is a 1?:1 relation between consumed messages and subsystems.
		pub trait AssociateOutgoing: ::std::fmt::Debug + Send {
			/// The associated _outgoing_ messages for a subsystem that _consumes_ the message `Self`.
			type OutgoingMessages: Into< #all_messages_wrapper > + ::std::fmt::Debug + Send;
		}

		// Helper for tests, where nothing is ever sent.
		impl AssociateOutgoing for () {
			type OutgoingMessages = ();
		}

		// Helper for tests, allows sending of arbitrary messages give
		// an test context.
		impl AssociateOutgoing for #all_messages_wrapper {
			type OutgoingMessages = #all_messages_wrapper ;
		}
	}
}

/// Implement `AssociateOutgoing` for `#consumes` being handled by a particular subsystem.
///
/// Binds the outgoing messages to the inbound message.
///
/// Note: Works, since there is a 1:1 relation between inbound message type and subsystem declarations.
/// Note: A workaround until default associated types work in `rustc`.
pub(crate) fn impl_associate_outgoing_messages(
	consumes: &Path,
	outgoing_wrapper: &Ident,
) -> TokenStream {
	quote! {
		impl AssociateOutgoing for #outgoing_wrapper {
			type OutgoingMessages = #outgoing_wrapper;
		}

		impl AssociateOutgoing for #consumes {
			type OutgoingMessages = #outgoing_wrapper;
		}
	}
}

/// Implement `trait SubsystemContext` for a particular subsystem context,
/// that is generated by the proc-macro too.
pub(crate) fn impl_subsystem_context_trait_for(
	consumes: Type,
	outgoing: &[Type],
	outgoing_wrapper: Type,
	all_messages_wrapper: &Ident,
	subsystem_ctx_name: &Ident,
	subsystem_sender_name: &Ident,
	support_crate: &Path,
	signal: &Path,
	error_ty: &Path,
) -> TokenStream {
	// impl the subsystem context trait
	let where_clause = quote! {
		#consumes: AssociateOutgoing + ::std::fmt::Debug + Send + 'static,
		#all_messages_wrapper: From< #outgoing_wrapper >,
		#all_messages_wrapper: From< #consumes >,
		#outgoing_wrapper: #( From< #outgoing > )+*,
	};

	quote! {
		#[#support_crate ::async_trait]
		impl #support_crate ::SubsystemContext for #subsystem_ctx_name < #consumes >
		where
			#where_clause
		{
			type Message = #consumes;
			type Signal = #signal;
			type OutgoingMessages = #outgoing_wrapper;
			type Sender = #subsystem_sender_name < #outgoing_wrapper >;
			type Error = #error_ty;

			async fn try_recv(&mut self) -> ::std::result::Result<Option<FromOrchestra< Self::Message, #signal>>, ()> {
				match #support_crate ::poll!(self.recv()) {
					#support_crate ::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
					#support_crate ::Poll::Pending => Ok(None),
				}
			}

			async fn recv(&mut self) -> ::std::result::Result<FromOrchestra<Self::Message, #signal>, #error_ty> {
				loop {
					// If we have a message pending an orchestra signal, we only poll for signals
					// in the meantime.
					if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
						if needs_signals_received <= self.signals_received.load() {
							return Ok( #support_crate ::FromOrchestra::Communication { msg });
						} else {
							self.pending_incoming = Some((needs_signals_received, msg));

							// wait for next signal.
							let signal = self.signals.next().await
								.ok_or(#support_crate ::OrchestraError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							self.signals_received.inc();
							return Ok( #support_crate ::FromOrchestra::Signal(signal))
						}
					}

					let mut await_message = self.messages.next().fuse();
					let mut await_signal = self.signals.next().fuse();
					let signals_received = self.signals_received.load();
					let pending_incoming = &mut self.pending_incoming;

					// Otherwise, wait for the next signal or incoming message.
					let from_orchestra = #support_crate ::futures::select_biased! {
						signal = await_signal => {
							let signal = signal
								.ok_or( #support_crate ::OrchestraError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							#support_crate ::FromOrchestra::Signal(signal)
						}
						msg = await_message => {
							let packet = msg
								.ok_or( #support_crate ::OrchestraError::Context(
									"Message channel is terminated and empty."
									.to_owned()
								))?;

							if packet.signals_received > signals_received {
								// wait until we've received enough signals to return this message.
								*pending_incoming = Some((packet.signals_received, packet.message));
								continue;
							} else {
								// we know enough to return this message.
								#support_crate ::FromOrchestra::Communication { msg: packet.message}
							}
						}
					};

					if let #support_crate ::FromOrchestra::Signal(_) = from_orchestra {
						self.signals_received.inc();
					}

					return Ok(from_orchestra);
				}
			}

			fn sender(&mut self) -> &mut Self::Sender {
				&mut self.to_subsystems
			}

			fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::std::result::Result<(), #error_ty>
			{
				self.to_orchestra.unbounded_send(#support_crate ::ToOrchestra::SpawnJob {
					name,
					subsystem: Some(self.name()),
					s,
				}).map_err(|_| #support_crate ::OrchestraError::TaskSpawn(name))?;
				Ok(())
			}

			fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::std::result::Result<(), #error_ty>
			{
				self.to_orchestra.unbounded_send(#support_crate ::ToOrchestra::SpawnBlockingJob {
					name,
					subsystem: Some(self.name()),
					s,
				}).map_err(|_| #support_crate ::OrchestraError::TaskSpawn(name))?;
				Ok(())
			}
		}
	}
}

/// Implement the additional subsystem accumulation traits, for simplified usage,
/// i.e. `${Subsystem}SenderTrait` and `${Subsystem}ContextTrait`.
pub(crate) fn impl_per_subsystem_helper_traits(
	info: &OrchestraInfo,
	subsystem_ctx_name: &Ident,
	subsystem_ctx_trait: &Ident,
	subsystem_sender_name: &Ident,
	subsystem_sender_trait: &Ident,
	consumes: &Path,
	outgoing: &[Path],
	outgoing_wrapper: &Ident,
) -> TokenStream {
	let all_messages_wrapper = &info.message_wrapper;
	let signal_ty = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let mut ts = TokenStream::new();

	// Create a helper trait bound of all outgoing messages, and the generated wrapper type
	// for ease of use within subsystems:
	let acc_sender_trait_bounds = quote! {
		#support_crate ::SubsystemSender< #outgoing_wrapper >
		#(
			+ #support_crate ::SubsystemSender< #outgoing >
		)*
			+ #support_crate ::SubsystemSender< () >
			+ Send
			+ 'static
	};

	ts.extend(quote! {
		/// A abstracting trait for usage with subsystems.
		pub trait #subsystem_sender_trait : #acc_sender_trait_bounds
		{}

		impl<T> #subsystem_sender_trait for T
		where
			T: #acc_sender_trait_bounds
		{}
	});

	// Create a helper accumulated per subsystem trait bound:
	let where_clause = quote! {
		#consumes: AssociateOutgoing + ::std::fmt::Debug + Send + 'static,
		#all_messages_wrapper: From< #outgoing_wrapper >,
		#all_messages_wrapper: From< #consumes >,
		#all_messages_wrapper: From< () >,
		#outgoing_wrapper: #( From< #outgoing > )+*,
		#outgoing_wrapper: From< () >,
	};

	ts.extend(quote! {
		/// Accumulative trait for a particular subsystem wrapper.
		pub trait #subsystem_ctx_trait : SubsystemContext <
			Message = #consumes,
			Signal = #signal_ty,
			OutgoingMessages = #outgoing_wrapper,
			// Sender,
			Error = #error_ty,
		>
		where
			#where_clause
			<Self as SubsystemContext>::Sender:
				#subsystem_sender_trait
				+ #acc_sender_trait_bounds,
		{
			/// Sender.
			type Sender: #subsystem_sender_trait;
		}

		impl<T> #subsystem_ctx_trait for T
		where
			T: SubsystemContext <
				Message = #consumes,
				Signal = #signal_ty,
				OutgoingMessages = #outgoing_wrapper,
				// Sender
				Error = #error_ty,
			>,
			#where_clause
			<T as SubsystemContext>::Sender:
				#subsystem_sender_trait
				+ #acc_sender_trait_bounds,
		{
			type Sender = <T as SubsystemContext>::Sender;
		}
	});

	ts.extend(impl_subsystem_context_trait_for(
		parse_quote! { #consumes },
		&Vec::from_iter(outgoing.iter().map(|path| {
			parse_quote! { #path }
		})),
		parse_quote! { #outgoing_wrapper },
		all_messages_wrapper,
		subsystem_ctx_name,
		subsystem_sender_name,
		support_crate,
		signal_ty,
		error_ty,
	));
	ts
}

/// Generate the subsystem context type and provide `fn new` on it.
///
/// Note: The generated `fn new` is used by the [builder pattern](../impl_builder.rs).
pub(crate) fn impl_subsystem_context(
	info: &OrchestraInfo,
	subsystem_sender_name: &Ident,
	subsystem_ctx_name: &Ident,
) -> TokenStream {
	let signal_ty = &info.extern_signal_ty;
	let support_crate = info.support_crate_name();

	let ts = quote! {
		/// A context type that is given to the [`Subsystem`] upon spawning.
		/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
		/// or to spawn it's [`SubsystemJob`]s.
		///
		/// [`Orchestra`]: struct.Orchestra.html
		/// [`Subsystem`]: trait.Subsystem.html
		/// [`SubsystemJob`]: trait.SubsystemJob.html
		#[derive(Debug)]
		#[allow(missing_docs)]
		pub struct #subsystem_ctx_name<M: AssociateOutgoing + Send + 'static> {
			signals: #support_crate ::metered::MeteredReceiver< #signal_ty >,
			messages: SubsystemIncomingMessages< M >,
			to_subsystems: #subsystem_sender_name < <M as AssociateOutgoing>::OutgoingMessages >,
			to_orchestra: #support_crate ::metered::UnboundedMeteredSender<
				#support_crate ::ToOrchestra
				>,
			signals_received: SignalsReceived,
			pending_incoming: Option<(usize, M)>,
			name: &'static str
		}

		impl<M> #subsystem_ctx_name <M>
		where
			M: AssociateOutgoing + Send + 'static,
		{
			/// Create a new context.
			fn new(
				signals: #support_crate ::metered::MeteredReceiver< #signal_ty >,
				messages: SubsystemIncomingMessages< M >,
				to_subsystems: ChannelsOut,
				to_orchestra: #support_crate ::metered::UnboundedMeteredSender<#support_crate:: ToOrchestra>,
				name: &'static str
			) -> Self {
				let signals_received = SignalsReceived::default();
				#subsystem_ctx_name :: <M> {
					signals,
					messages,
					to_subsystems: #subsystem_sender_name :: < <M as AssociateOutgoing>::OutgoingMessages > {
						channels: to_subsystems,
						signals_received: signals_received.clone(),
						_phantom: ::core::marker::PhantomData::default(),
					},
					to_orchestra,
					signals_received,
					pending_incoming: None,
					name
				}
			}

			fn name(&self) -> &'static str {
				self.name
			}
		}
	};

	ts
}
