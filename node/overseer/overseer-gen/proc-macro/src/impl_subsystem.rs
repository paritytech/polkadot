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

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Path, Result};

use petgraph::{
	dot::{self, Dot},
	graph::NodeIndex,
	visit::EdgeRef,
	Direction,
};
use std::collections::HashMap;

use super::*;

/// Render a graphviz (aka dot graph) to a file.
fn graphviz(
	graph: &petgraph::Graph<Ident, Path>,
	dest: &mut impl std::io::Write,
) -> std::io::Result<()> {
	let config = &[dot::Config::EdgeNoLabel, dot::Config::NodeNoLabel][..];
	let dot = Dot::with_attr_getters(
		graph,
		config,
		&|_graph, edge| -> String {
			format!(
				r#"label="{}""#,
				edge.weight().get_ident().expect("Must have a trailing identifier. qed")
			)
		},
		&|_graph, (_node_index, subsystem_name)| -> String {
			format!(r#"label="{}""#, subsystem_name,)
		},
	);
	dest.write_all(format!("{:?}", &dot).as_bytes())?;
	Ok(())
}

pub(crate) fn impl_subsystem(info: &OverseerInfo) -> Result<TokenStream> {
	let mut ts = TokenStream::new();

	let overseer_name = &info.overseer_name;
	let span = overseer_name.span();
	let all_messages_wrapper = &info.message_wrapper;
	let support_crate = info.support_crate_name();

	// create a directed graph with all the subsystems as nodes and the messages as edges
	// key is always the message path, values are node indices in the graph and the subsystem generic identifier
	// store the message path and the source sender, both in the graph as well as identifier
	let mut outgoing_lut = HashMap::<&Path, Vec<(Ident, NodeIndex)>>::with_capacity(128);
	// same for consuming the incoming messages
	let mut consuming_lut = HashMap::<&Path, (Ident, NodeIndex)>::with_capacity(128);

	// Ident = Node = subsystem generic names
	// Path = Edge = messages
	let mut graph = petgraph::Graph::<Ident, Path>::new();

	// prepare the full index of outgoing and source subsystems
	for ssf in info.subsystems() {
		let node_index = graph.add_node(ssf.generic.clone());
		for outgoing in ssf.messages_to_send.iter() {
			outgoing_lut
				.entry(outgoing)
				.or_default()
				.push((ssf.generic.clone(), node_index));
		}
		consuming_lut.insert(&ssf.message_to_consume, (ssf.generic.clone(), node_index));
	}

	for (message_ty, (_consuming_subsystem_ident, consuming_node_index)) in consuming_lut.iter() {
		// match the outgoing ones that were registered above with the consumed message
		if let Some(origin_subsystems) = outgoing_lut.get(message_ty) {
			for (_origin_subsystem_ident, sending_node_index) in origin_subsystems.iter() {
				graph.add_edge(*sending_node_index, *consuming_node_index, (*message_ty).clone());
			}
		}
	}

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
					}
				}
			}
		})
	}

	// Dump the graph to file.
	if cfg!(feature = "graph") || true {
		let path = std::path::PathBuf::from(env!("OUT_DIR"))
			.join(overseer_name.to_string().to_lowercase() + "-subsystem-messaging.dot");
		if let Err(e) = std::fs::OpenOptions::new()
			.truncate(true)
			.create(true)
			.write(true)
			.open(&path)
			.and_then(|mut f| graphviz(&graph, &mut f))
		{
			eprintln!("Failed to write dot graph to {}: {:?}", path.display(), e);
		} else {
			println!("Wrote dot graph to {}", path.display());
		}
	}

	let subsystem_sender_name = &Ident::new(&(overseer_name.to_string() + "Sender"), span);
	let subsystem_ctx_name = &Ident::new(&(overseer_name.to_string() + "SubsystemContext"), span);
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

pub(crate) fn impl_subsystem_sender(
	support_crate: &TokenStream,
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
	// 2. overseer-global-`AllMessages`-type
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

		impl AssociateOutgoing for () {
			type OutgoingMessages = ();
		}
	}
}

/// A workaround until default associated types actually work.
pub(crate) fn impl_associate_outgoing_messages(
	message: &Path,
	outgoing_wrapper: &Ident,
) -> TokenStream {
	quote! {
		impl AssociateOutgoing for #outgoing_wrapper {
			type OutgoingMessages = #outgoing_wrapper;
		}

		impl AssociateOutgoing for #message {
			type OutgoingMessages = #outgoing_wrapper;
		}
	}
}

pub(crate) fn impl_per_subsystem_helper_traits(
	info: &OverseerInfo,
	subsystem_ctx_trait: &Ident,
	subsystem_sender_name: &Ident,
	subsystem_sender_trait: &Ident,
	consumes: &Path,
	outgoing: &[Path],
	outgoing_wrapper: &Ident,
) -> TokenStream {
	let all_messages_wrapper = &info.message_wrapper;
	let signal = &info.extern_signal_ty;
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
			+ Send
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
		<Self as SubsystemContext>::Sender: #subsystem_sender_trait,
		<Self as SubsystemContext>::Sender: #acc_sender_trait_bounds,
	};

	ts.extend(quote! {
		/// Accumuative trait for a particular subsystem wrapper.
		pub trait #subsystem_ctx_trait : SubsystemContext <
			Message = #consumes,
			Signal = #signal,
			OutgoingMessages = #outgoing_wrapper,
			// Sender,
			Error = #error_ty,
		>
		where
			#where_clause
		{
			/// Sender.
			type Sender: #subsystem_sender_trait + #acc_sender_trait_bounds;
		}

		impl<T> #subsystem_ctx_trait for T
		where
			T: SubsystemContext <
				Message = #consumes,
				Signal = #signal,
				OutgoingMessages = #outgoing_wrapper,
				// Sender
				Error = #error_ty,
			>,
			#where_clause
		{
			type Sender = <Self as SubsystemContext>::Sender;
		}
	});

	// // impl the subsystem context trait (alt!)
	// ts.extend(quote!{

	// 	#[#support_crate ::async_trait]
	// 	impl #support_crate ::SubsystemContext for #subsystem_ctx_name <#consumes>
	// 	where
	// 		#consumes: AssociateOutgoing,
	// 		// : <#consumes as AssociateOutgoing>::OutgoingMessages,
	// 		#all_messages_wrapper: From< #consumes >,
	// 		#subsystem_sender_name:
	// 			#support_crate ::SubsystemSender< #outgoing_wrapper >
	// 			#(
	// 				+ #support_crate ::SubsystemSender< #outgoing >
	// 			)*
	// 			,
	// 	{
	// 		type Message = #consumes;
	// 		type Signal = #signal;
	// 		type OutgoingMessages = #outgoing_wrapper;
	// 		// type AllMessages = #all_messages_wrapper;
	// 		type Sender = #subsystem_sender_name;
	// 		type Error = #error_ty;

	// 		async fn try_recv(&mut self) -> ::std::result::Result<Option<FromOverseer<M, Self::Signal>>, ()> {
	// 			match #support_crate ::poll!(self.recv()) {
	// 				#support_crate ::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
	// 				#support_crate ::Poll::Pending => Ok(None),
	// 			}
	// 		}

	// 		async fn recv(&mut self) -> ::std::result::Result<FromOverseer<M, Self::Signal>, Self::Error> {
	// 			loop {
	// 				// If we have a message pending an overseer signal, we only poll for signals
	// 				// in the meantime.
	// 				if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
	// 					if needs_signals_received <= self.signals_received.load() {
	// 						return Ok( #support_crate ::FromOverseer::Communication { msg });
	// 					} else {
	// 						self.pending_incoming = Some((needs_signals_received, msg));

	// 						// wait for next signal.
	// 						let signal = self.signals.next().await
	// 							.ok_or(#support_crate ::OverseerError::Context(
	// 								"Signal channel is terminated and empty."
	// 								.to_owned()
	// 							))?;

	// 						self.signals_received.inc();
	// 						return Ok( #support_crate ::FromOverseer::Signal(signal))
	// 					}
	// 				}

	// 				let mut await_message = self.messages.next().fuse();
	// 				let mut await_signal = self.signals.next().fuse();
	// 				let signals_received = self.signals_received.load();
	// 				let pending_incoming = &mut self.pending_incoming;

	// 				// Otherwise, wait for the next signal or incoming message.
	// 				let from_overseer = #support_crate ::futures::select_biased! {
	// 					signal = await_signal => {
	// 						let signal = signal
	// 							.ok_or( #support_crate ::OverseerError::Context(
	// 								"Signal channel is terminated and empty."
	// 								.to_owned()
	// 							))?;

	// 						#support_crate ::FromOverseer::Signal(signal)
	// 					}
	// 					msg = await_message => {
	// 						let packet = msg
	// 							.ok_or( #support_crate ::OverseerError::Context(
	// 								"Message channel is terminated and empty."
	// 								.to_owned()
	// 							))?;

	// 						if packet.signals_received > signals_received {
	// 							// wait until we've received enough signals to return this message.
	// 							*pending_incoming = Some((packet.signals_received, packet.message));
	// 							continue;
	// 						} else {
	// 							// we know enough to return this message.
	// 							#support_crate ::FromOverseer::Communication { msg: packet.message}
	// 						}
	// 					}
	// 				};

	// 				if let #support_crate ::FromOverseer::Signal(_) = from_overseer {
	// 					self.signals_received.inc();
	// 				}

	// 				return Ok(from_overseer);
	// 			}
	// 		}

	// 		fn sender(&mut self) -> &mut Self::Sender {
	// 			&mut self.to_subsystems
	// 		}

	// 		fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
	// 			-> ::std::result::Result<(), #error_ty>
	// 		{
	// 			self.to_overseer.unbounded_send(#support_crate ::ToOverseer::SpawnJob {
	// 				name,
	// 				subsystem: Some(self.name()),
	// 				s,
	// 			}).map_err(|_| #support_crate ::OverseerError::TaskSpawn(name))?;
	// 			Ok(())
	// 		}

	// 		fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
	// 			-> ::std::result::Result<(), #error_ty>
	// 		{
	// 			self.to_overseer.unbounded_send(#support_crate ::ToOverseer::SpawnBlockingJob {
	// 				name,
	// 				subsystem: Some(self.name()),
	// 				s,
	// 			}).map_err(|_| #support_crate ::OverseerError::TaskSpawn(name))?;
	// 			Ok(())
	// 		}
	// 	}
	// });
	ts
}

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
pub(crate) fn impl_subsystem_context(
	info: &OverseerInfo,
	subsystem_sender_name: &Ident,
	subsystem_ctx_name: &Ident,
) -> TokenStream {
	let all_messages_wrapper = &info.message_wrapper;
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let mut ts = quote! {
		/// A context type that is given to the [`Subsystem`] upon spawning.
		/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
		/// or to spawn it's [`SubsystemJob`]s.
		///
		/// [`Overseer`]: struct.Overseer.html
		/// [`Subsystem`]: trait.Subsystem.html
		/// [`SubsystemJob`]: trait.SubsystemJob.html
		#[derive(Debug)]
		#[allow(missing_docs)]
		pub struct #subsystem_ctx_name<M: AssociateOutgoing + Send + 'static> {
			signals: #support_crate ::metered::MeteredReceiver< #signal >,
			messages: SubsystemIncomingMessages< M >,
			to_subsystems: #subsystem_sender_name < <M as AssociateOutgoing>::OutgoingMessages >,
			to_overseer: #support_crate ::metered::UnboundedMeteredSender<
				#support_crate ::ToOverseer
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
				signals: #support_crate ::metered::MeteredReceiver< #signal >,
				messages: SubsystemIncomingMessages< M >,
				to_subsystems: ChannelsOut,
				to_overseer: #support_crate ::metered::UnboundedMeteredSender<#support_crate:: ToOverseer>,
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
					to_overseer,
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

	// impl the subsystem context trait
	ts.extend(quote! {

		#[#support_crate ::async_trait]
		impl<M> #support_crate ::SubsystemContext for #subsystem_ctx_name <M>
		where
			M: AssociateOutgoing,
			#all_messages_wrapper: From<M>,
			#subsystem_sender_name<
				<M as AssociateOutgoing>::OutgoingMessages
			>:
				#support_crate ::SubsystemSender<
					<M as AssociateOutgoing>::OutgoingMessages
				>,
		{
			type Message = M;
			type Signal = #signal;
			type OutgoingMessages = <M as AssociateOutgoing>::OutgoingMessages;
			// type AllMessages = #all_messages_wrapper;
			type Sender = #subsystem_sender_name < <M as AssociateOutgoing>::OutgoingMessages >;
			type Error = #error_ty;

			async fn try_recv(&mut self) -> ::std::result::Result<Option<FromOverseer<M, #signal>>, ()> {
				match #support_crate ::poll!(self.recv()) {
					#support_crate ::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
					#support_crate ::Poll::Pending => Ok(None),
				}
			}

			async fn recv(&mut self) -> ::std::result::Result<FromOverseer<M, #signal>, #error_ty> {
				loop {
					// If we have a message pending an overseer signal, we only poll for signals
					// in the meantime.
					if let Some((needs_signals_received, msg)) = self.pending_incoming.take() {
						if needs_signals_received <= self.signals_received.load() {
							return Ok( #support_crate ::FromOverseer::Communication { msg });
						} else {
							self.pending_incoming = Some((needs_signals_received, msg));

							// wait for next signal.
							let signal = self.signals.next().await
								.ok_or(#support_crate ::OverseerError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							self.signals_received.inc();
							return Ok( #support_crate ::FromOverseer::Signal(signal))
						}
					}

					let mut await_message = self.messages.next().fuse();
					let mut await_signal = self.signals.next().fuse();
					let signals_received = self.signals_received.load();
					let pending_incoming = &mut self.pending_incoming;

					// Otherwise, wait for the next signal or incoming message.
					let from_overseer = #support_crate ::futures::select_biased! {
						signal = await_signal => {
							let signal = signal
								.ok_or( #support_crate ::OverseerError::Context(
									"Signal channel is terminated and empty."
									.to_owned()
								))?;

							#support_crate ::FromOverseer::Signal(signal)
						}
						msg = await_message => {
							let packet = msg
								.ok_or( #support_crate ::OverseerError::Context(
									"Message channel is terminated and empty."
									.to_owned()
								))?;

							if packet.signals_received > signals_received {
								// wait until we've received enough signals to return this message.
								*pending_incoming = Some((packet.signals_received, packet.message));
								continue;
							} else {
								// we know enough to return this message.
								#support_crate ::FromOverseer::Communication { msg: packet.message}
							}
						}
					};

					if let #support_crate ::FromOverseer::Signal(_) = from_overseer {
						self.signals_received.inc();
					}

					return Ok(from_overseer);
				}
			}

			fn sender(&mut self) -> &mut Self::Sender {
				&mut self.to_subsystems
			}

			fn spawn(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::std::result::Result<(), #error_ty>
			{
				self.to_overseer.unbounded_send(#support_crate ::ToOverseer::SpawnJob {
					name,
					subsystem: Some(self.name()),
					s,
				}).map_err(|_| #support_crate ::OverseerError::TaskSpawn(name))?;
				Ok(())
			}

			fn spawn_blocking(&mut self, name: &'static str, s: Pin<Box<dyn Future<Output = ()> + Send>>)
				-> ::std::result::Result<(), #error_ty>
			{
				self.to_overseer.unbounded_send(#support_crate ::ToOverseer::SpawnBlockingJob {
					name,
					subsystem: Some(self.name()),
					s,
				}).map_err(|_| #support_crate ::OverseerError::TaskSpawn(name))?;
				Ok(())
			}
		}
	});

	ts
}
