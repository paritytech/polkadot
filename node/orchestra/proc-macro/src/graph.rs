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

use syn::{Ident, Path};

use petgraph::{
	dot::{self, Dot},
	graph::NodeIndex,
	visit::IntoEdgeReferences,
	Graph,
};
use std::collections::{hash_map::RandomState, HashMap, HashSet};

use super::*;

/// Representation of all subsystem connections
pub(crate) struct ConnectionGraph<'a> {
	// Ident = Node = subsystem generic names
	// Path = Edge = messages
	/// Graph of connected subsystems
	pub(crate) graph: Graph<Ident, Path>,
	/// Cycles within the graph
	pub(crate) cycles: Vec<Vec<NodeIndex>>,
	/// Messages that are never being sent (and by which subsystem), but are consumed
	/// Maps the message `Path` to the subsystem `Ident` represented by `NodeIndex`.
	pub(crate) unsent_messages: HashMap<&'a Path, (&'a Ident, NodeIndex)>,
	/// Messages being sent (and by which subsystem), but not consumed by any subsystem
	/// Maps the message `Path` to the subsystem `Ident` represented by `NodeIndex`.
	pub(crate) unconsumed_messages: HashMap<&'a Path, Vec<(&'a Ident, NodeIndex)>>,
}

impl<'a> ConnectionGraph<'a> {
	/// Generates all subsystem types and related accumulation traits.
	pub(crate) fn construct(ssfs: &'a [SubSysField]) -> Self {
		// create a directed graph with all the subsystems as nodes and the messages as edges
		// key is always the message path, values are node indices in the graph and the subsystem generic identifier
		// store the message path and the source sender, both in the graph as well as identifier
		let mut outgoing_lut = HashMap::<&Path, Vec<(&Ident, NodeIndex)>>::with_capacity(128);
		// same for consuming the incoming messages
		let mut consuming_lut = HashMap::<&Path, (&Ident, NodeIndex)>::with_capacity(128);

		let mut graph = Graph::<Ident, Path>::new();

		// prepare the full index of outgoing and source subsystems
		for ssf in ssfs {
			let node_index = graph.add_node(ssf.generic.clone());
			for outgoing in ssf.messages_to_send.iter() {
				outgoing_lut
					.entry(outgoing)
					.or_default()
					.push((&ssf.generic, node_index));
			}
			if let Some(_first_consument) = consuming_lut.insert(&ssf.message_to_consume, (&ssf.generic, node_index)) {
				// bail, two subsystems consuming the same message
			}
		}

		for (message_ty, (_consuming_subsystem_ident, consuming_node_index)) in consuming_lut.iter()
		{
			// match the outgoing ones that were registered above with the consumed message
			if let Some(origin_subsystems) = outgoing_lut.get(message_ty) {
				for (_origin_subsystem_ident, sending_node_index) in origin_subsystems.iter() {
					graph.add_edge(
						*sending_node_index,
						*consuming_node_index,
						(*message_ty).clone(),
					);
				}
			}
		}

		// extract unsent and unreceived messages
		let outgoing_set = HashSet::<_, RandomState>::from_iter(outgoing_lut.keys().cloned());
		let consuming_set = HashSet::<_, RandomState>::from_iter(consuming_lut.keys().cloned());

		let mut unsent_messages = consuming_lut;
		unsent_messages.retain(|k, _v| !outgoing_set.contains(k));

		let mut unconsumed_messages = outgoing_lut;
		unconsumed_messages.retain(|k, _v| !consuming_set.contains(k));

		// Extract the cycles
		let cycles = petgraph::algo::kosaraju_scc(&graph);
		match cycles.len() {
			0 => println!("Found zer0 cycles"),
			1 => println!("Found 1 cycle"),
			n => println!("Found {n} cycles"),
		}
		Self { graph, cycles, unsent_messages, unconsumed_messages }
	}

	/// Render a graphviz (aka dot graph) to a file.
	pub(crate) fn graphviz(self, dest: &mut impl std::io::Write) -> std::io::Result<()> {
		let config = &[dot::Config::EdgeNoLabel, dot::Config::NodeNoLabel][..];

		let Self { mut graph, unsent_messages, unconsumed_messages, cycles: _ } = self;

		// Adding nodes is ok, the `NodeIndex` is append only as long
		// there are no removals.

		// Depict sink for unconsumed messages
		let unconsumed_idx = graph.add_node(quote::format_ident!("SENT_TO_NONONE"));
		for (message_name, subsystems) in unconsumed_messages {
			// iterate over all subsystems that send such a message
			for (_sub_name, sub_node_idx) in subsystems {
				graph.add_edge(sub_node_idx, unconsumed_idx, message_name.clone());
			}
		}

		// depict source of unsent message, this is legit when
		// integrated with an external source, and sending messages based
		// on that
		let unsent_idx = graph.add_node(quote::format_ident!("NEVER_SENT_ANYWHERE"));
		for (message_name, (_sub_name, sub_node_idx)) in unsent_messages {
			graph.add_edge(unsent_idx, sub_node_idx, message_name.clone());
		}

		// TODO adjust rendering for cycles
		let unsent_edge_label = r#"label="⛲",fillcolor=darkgreen,shape=doublecircle,style=filled,fontname="NotoColorEmoji""#;
		let unconsumed_edge_label = r#"label="💀",fillcolor=black,shape=doublecircle,style=filled,fontname="NotoColorEmoji""#;
		let edge_attr = move |_graph: &Graph<Ident, Path>,
		                      edge: <&Graph<Ident, Path> as IntoEdgeReferences>::EdgeRef|
		      -> String {
			format!(
				r#"label="{}""#,
				edge.weight().get_ident().expect("Must have a trailing identifier. qed")
			)
		};
		let node_attr = |_graph, (node_index, subsystem_name)| -> String {
			if node_index == unsent_idx {
				unsent_edge_label.to_owned().clone()
			} else if node_index == unconsumed_idx {
				unconsumed_edge_label.to_owned().clone()
			} else {
				format!(r#"label="{}""#, subsystem_name,)
			}
		};
		let dot = Dot::with_attr_getters(
			&graph, config, &edge_attr, // with state, the reference is a trouble maker
			&node_attr,
		);
		dest.write_all(format!("{:?}", &dot).as_bytes())?;
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// whenever this starts working, we should consider
	// replacing the all caps idents with something like
	// the below.
	// <https://rust-lang.github.io/rfcs/2457-non-ascii-idents.html>
	//
	// For now the rendering is modified, the ident is a placeholder.
	#[test]
	#[should_panic]
	fn check_ident() {
		let _ident = quote::format_ident!("x💀x");
	}
}
