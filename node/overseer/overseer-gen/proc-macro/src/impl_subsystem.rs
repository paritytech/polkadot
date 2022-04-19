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
		&|_graph, (node_index, subsystem_name)| -> String {
			format!(r#"label="{}""#, subsystem_name,)
		},
	);
	dest.write_all(format!("{:?}", &dot).as_bytes())?;
	Ok(())
}

pub(crate) fn impl_subsystem(info: &OverseerInfo) -> Result<TokenStream> {
	let mut ts = TokenStream::new();

	let span = info.overseer_name.span();
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

	for (message_ty, (consuming_subsystem_ident, consuming_node_index)) in consuming_lut.iter() {
		// match the outgoing ones that were registered above with the consumed message
		if let Some(origin_subsystems) = outgoing_lut.get(message_ty) {
			for (origin_subsystem, sending_node_index) in origin_subsystems.iter() {
				graph.add_edge(*sending_node_index, *consuming_node_index, (*message_ty).clone());
			}
		}
	}

	// all outgoing edges are now usable to deriver everything we need
	for node_index in graph.node_indices() {
		let subsystem_name = graph[node_index].to_string();
		let outgoing_wrapper = Ident::new(&(subsystem_name + "OutgoingMessages"), span);

		// cannot be a hashmap, duplicate keys and sorting required
		let outgoing_to_consumer = graph
			.edges_directed(node_index, Direction::Outgoing)
			.map(|edge| {
				let message_ty = edge.weight();
				let subsystem_generic_consumer = graph[edge.target()].clone();
				Ok((to_variant(message_ty, span.clone())?, subsystem_generic_consumer))
			})
			.collect::<Result<Vec<(Ident, Ident)>>>()?;

		let outgoing_variant = outgoing_to_consumer.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
		let subsystem_generic = outgoing_to_consumer.into_iter().map(|x| x.1).collect::<Vec<_>>();

		ts.extend(quote! {
			impl ::std::convert::From< #outgoing_wrapper > for #all_messages_wrapper {
				fn from(message: #outgoing_wrapper) -> Self {
					match message {
					#(
						#outgoing_wrapper :: #outgoing_variant ( msg ) => #all_messages_wrapper :: #subsystem_generic ( msg ),
					)*
					}
				}
			}
		})
	}

	if true {
		let mut f = std::fs::OpenOptions::new()
			.truncate(true)
			.create(true)
			.write(true)
			.open("/tmp/foo.dot")
			.expect("Opening that file works. qed");
		graphviz(&graph, &mut f).expect("Write of dot graph should work. qed");
	}

	for ssf in info.subsystems() {
		let subsystem_name = ssf.generic.to_string();
		let subsystem_sender_name = Ident::new(&(subsystem_name.clone() + "Sender"), span);
		let subsystem_ctx_name = Ident::new(&(subsystem_name.clone() + "Context"), span);

		let outgoing_wrapper = Ident::new(&(subsystem_name.clone() + "OutgoingMessages"), span);
		let consumes = ssf
			.message_to_consume
			.get_ident()
			.cloned()
			.unwrap_or_else(|| Ident::new("()", Span::call_site()));

		ts.extend(impl_wrapper_enum(&outgoing_wrapper, ssf.messages_to_send.as_slice())?);

		ts.extend(impl_subsystem_sender(
			ssf,
			&all_messages_wrapper,
			support_crate,
			&outgoing_wrapper,
			&all_messages_wrapper,
			&subsystem_sender_name,
		));
		ts.extend(impl_subsystem_context(
			info,
			&consumes,
			&outgoing_wrapper,
			&subsystem_sender_name,
			&subsystem_ctx_name,
		));
	}
	Ok(ts)
}

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
		#[derive(Clone)]
		pub enum #wrapper {
			#(
				#variants ( #message_types ),
			)*
		}

		#(
			impl ::std::convert::From< #message_types > for #wrapper {
				fn from(message: #message_types) -> Self {
					#wrapper :: #variants ( message )
				}
			}
		)*
	};
	Ok(ts)
}

/// Generates the wrapper type enum, no bells or whistles.
pub(crate) fn impl_wrapper_to_wrapper_glue(
	all_message: &Ident,
	wrapper: &Ident,
	message_types: &[Path],
) -> Result<TokenStream> {
	// The message types are path based, each of them must finish with a type
	// and as such we do this upfront.
	let variants: Vec<_> = Result::from_iter(message_types.into_iter().map(|path| {
		let ident = path
			.segments
			.last()
			.ok_or_else(|| {
				syn::Error::new(wrapper.span(), "Path is empty, but it must end with an identifier")
			})
			.map(|segment| segment.ident.clone());
		ident
	}))?;
	let ts = quote! {
		#[allow(missing_docs)]
		#[derive(Clone)]
		pub enum #wrapper {
			#(
				#variants ( #message_types ),
			)*
		}

		#(
			impl ::std::convert::From< #message_types > for #wrapper {
				fn from(message: #message_types) -> Self {
					#wrapper :: #variants ( message )
				}
			}
		)*
	};
	Ok(ts)
}

pub(crate) fn impl_subsystem_sender(
	ssf: &SubSysField,
	wrapper_message: &Ident,
	support_crate: &TokenStream,
	outgoing_wrapper: &Ident,
	all_messages_wrapper: &Ident,
	subsystem_sender_name: &Ident,
) -> TokenStream {
	let mut ts = quote! {
		/// Connector to send messages towards all subsystems,
		/// while tracking the which signals where already received.
		#[derive(Debug, Clone)]
		pub struct #subsystem_sender_name {
			/// Collection of channels to all subsystems.
			channels: ChannelsOut,
			/// Systemwide tick for which signals were received by all subsystems.
			signals_received: SignalsReceived,
		}
	};

	// Create the same for a wrapping enum:
	//
	// 1. subsystem specific `*OutgoingMessages`-type
	// 2. overseer-global-`AllMessages`-type
	let wrapped = |outgoing_wrapper: &Ident| {
		quote! {
			/// implementation for wrapping message type...
			#[#support_crate ::async_trait]
			impl<Message> SubsystemSender< Message > for #subsystem_sender_name
			where
				Message: Send + 'static,
				#outgoing_wrapper: ::std::convert::From<Message>,
				#all_messages_wrapper: ::std::convert::From<Message>,
			{
				async fn send_message(&mut self, msg: Message) {
					self.channels.send_and_log_error(
						self.signals_received.load(),
						#all_messages_wrapper ::from (
							#outgoing_wrapper :: from ( msg )
						)
					).await;
				}

				async fn send_messages<I>(&mut self, msgs: I)
				where
					I: IntoIterator<Item=Message> + Send,
					I::IntoIter: Iterator<Item=Message>,
				{
					for msg in msgs {
						self.send_message( msg );
					}
				}

				fn send_unbounded_message(&mut self, msg: Message) {
					self.channels.send_unbounded_and_log_error(
						self.signals_received.load(),
						#all_messages_wrapper ::from (
							#outgoing_wrapper :: from ( msg )
						)
					);
				}
			}
		}
	};

	// Allow the `#wrapper_message` to be sent as well.
	ts.extend(wrapped(&outgoing_wrapper));

	// TODO FIXME
	let inconsequent = false;
	if inconsequent {
		ts.extend(wrapped(wrapper_message));
	}

	ts
}

/// Implement a builder pattern for the `Overseer`-type,
/// which acts as the gateway to constructing the overseer.
pub(crate) fn impl_subsystem_context(
	info: &OverseerInfo,
	consumes: &Ident,
	sends: &Ident,
	subsystem_sender_name: &Ident,
	subsystem_ctx_name: &Ident,
) -> TokenStream {
	let signal = &info.extern_signal_ty;
	let error_ty = &info.extern_error_ty;
	let support_crate = info.support_crate_name();

	let ts = quote! {
		/// A context type that is given to the [`Subsystem`] upon spawning.
		/// It can be used by [`Subsystem`] to communicate with other [`Subsystem`]s
		/// or to spawn it's [`SubsystemJob`]s.
		///
		/// [`Overseer`]: struct.Overseer.html
		/// [`Subsystem`]: trait.Subsystem.html
		/// [`SubsystemJob`]: trait.SubsystemJob.html
		#[derive(Debug)]
		#[allow(missing_docs)]
		pub struct #subsystem_ctx_name {
			signals: #support_crate ::metered::MeteredReceiver< #signal >,
			messages: SubsystemIncomingMessages< #consumes >,
			to_subsystems: #subsystem_sender_name,
			to_overseer: #support_crate ::metered::UnboundedMeteredSender<
				#support_crate ::ToOverseer
				>,
			signals_received: SignalsReceived,
			pending_incoming: Option<(usize, #consumes)>,
			name: &'static str
		}

		impl #subsystem_ctx_name {
			/// Create a new context.
			fn new(
				signals: #support_crate ::metered::MeteredReceiver< #signal >,
				messages: SubsystemIncomingMessages< #consumes >,
				to_subsystems: ChannelsOut,
				to_overseer: #support_crate ::metered::UnboundedMeteredSender<#support_crate:: ToOverseer>,
				name: &'static str
			) -> Self {
				let signals_received = SignalsReceived::default();
				#subsystem_ctx_name {
					signals,
					messages,
					to_subsystems: #subsystem_sender_name {
						channels: to_subsystems,
						signals_received: signals_received.clone(),
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

		#[#support_crate ::async_trait]
		impl #support_crate ::SubsystemContext for #subsystem_ctx_name
		where
			#subsystem_sender_name: #support_crate ::SubsystemSender< #sends >,
		{
			type Message = #consumes;
			type Signal = #signal;
			type Sender = #subsystem_sender_name;
			type OutgoingMessages = #sends;
			type Error = #error_ty;

			async fn try_recv(&mut self) -> ::std::result::Result<Option<FromOverseer<#consumes, #signal>>, ()> {
				match #support_crate ::poll!(self.recv()) {
					#support_crate ::Poll::Ready(msg) => Ok(Some(msg.map_err(|_| ())?)),
					#support_crate ::Poll::Pending => Ok(None),
				}
			}

			async fn recv(&mut self) -> ::std::result::Result<FromOverseer<#consumes, #signal>, #error_ty> {
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
	};

	ts
}
