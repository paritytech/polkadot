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

use petgraph::{
	dot::{self, Dot},
	graph::NodeIndex,
	visit::EdgeRef,
	Direction, Graph,
};
use std::collections::HashMap;

use super::*;

/// Representation of all subsystem connections
pub(crate) struct ConnectionGraph {
	// Ident = Node = subsystem generic names
	// Path = Edge = messages
	/// Graph of connected subsystems
	pub(crate) graph: Graph<Ident, Path>,
	/// Messages that are never being sent, but are consumed
	pub(crate) unsent_messages: Vec<Ident>,
	/// Messages being sent, but never received
	pub(crate) unreceived_messages: Vec<Ident>,
}

impl ConnectionGraph {
	/// Generates all subsystem types and related accumulation traits.
	pub(crate) fn construct(ssfs: &[SubSysField]) -> Result<Self> {
		// create a directed graph with all the subsystems as nodes and the messages as edges
		// key is always the message path, values are node indices in the graph and the subsystem generic identifier
		// store the message path and the source sender, both in the graph as well as identifier
		let mut outgoing_lut = HashMap::<&Path, Vec<(Ident, NodeIndex)>>::with_capacity(128);
		// same for consuming the incoming messages
		let mut consuming_lut = HashMap::<&Path, (Ident, NodeIndex)>::with_capacity(128);

		let mut graph = Graph::<Ident, Path>::new();

		// prepare the full index of outgoing and source subsystems
		for ssf in ssfs {
			let node_index = graph.add_node(ssf.generic.clone());
			for outgoing in ssf.messages_to_send.iter() {
				outgoing_lut
					.entry(outgoing)
					.or_default()
					.push((ssf.generic.clone(), node_index));
			}
			consuming_lut.insert(&ssf.message_to_consume, (ssf.generic.clone(), node_index));
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

		Ok(Self {
			graph,
			unsent_messages: vec![],     // TODO fill with data
			unreceived_messages: vec![], // TODO fill with data
		})
	}

	/// Render a graphviz (aka dot graph) to a file.
	pub(crate) fn graphviz(&self, dest: &mut impl std::io::Write) -> std::io::Result<()> {
		// TODO render unconnected
		// TODO highlight unconnected nodes
		// TODO highlight circles
		let config = &[dot::Config::EdgeNoLabel, dot::Config::NodeNoLabel][..];
		let dot = Dot::with_attr_getters(
			&self.graph,
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
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn foo() {}
}
