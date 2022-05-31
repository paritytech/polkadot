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

use quote::ToTokens;
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
	/// Graph of connected subsystems
	///
	/// The graph represents a subsystem as a node or `NodeIndex`
	/// and edges are messages sent, directed from the sender to
	/// the receiver of the message.
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
				outgoing_lut.entry(outgoing).or_default().push((&ssf.generic, node_index));
			}
			if let Some(_first_consument) =
				consuming_lut.insert(&ssf.message_to_consume, (&ssf.generic, node_index))
			{
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

		let cycles = Self::extract_cycles(&graph);

		Self { graph, cycles, unsent_messages, unconsumed_messages }
	}

	/// Extract the cycles and print them
	fn extract_cycles(graph: &Graph<Ident, Path>) -> Vec<Vec<NodeIndex>> {
		use petgraph::visit::EdgeRef;

		// there is no guarantee regarding the node indices in the individual cycles
		let cycles = petgraph::algo::kosaraju_scc(&graph);
		let cycles = Vec::from_iter(cycles.into_iter().filter(|cycle| {
			match cycle.len() {
				1 => {
					// contains cycles of length one,
					// which do not exists, might be an upstream bug?
					let node_idx = cycle[0];
					graph
						.edges_directed(node_idx, petgraph::Direction::Outgoing)
						.find(|edge| edge.target() == node_idx)
						.is_some()
				},
				0 => false,
				_n => true,
			}
		}));
		match cycles.len() {
			0 => println!("Found zer0 cycles"),
			1 => println!("Found 1 cycle"),
			n => println!("Found {n} cycles"),
		}

		let greek_alphabet = greek_alphabet();

		for (cycle_idx, cycle) in cycles.iter().enumerate() {
			let cycle_tag = greek_alphabet.get(cycle_idx).copied().unwrap_or('_');
			let mut acc = Vec::with_capacity(cycle.len());
			let first = cycle[0].clone();
			let mut node_idx = first;
			let print_idx = cycle_idx + 1;
			// track which ones were visited and which step
			// the step is required to truncate the output
			let mut visited = HashMap::new();
			for step in 0..cycle.len() {
				if let Some(edge) =
					graph.edges_directed(node_idx, petgraph::Direction::Outgoing).find(|edge| {
						cycle
							.iter()
							.find(|&cycle_node_idx| *cycle_node_idx == edge.target())
							.is_some()
					}) {
					let next = edge.target();
					visited.insert(node_idx, step);

					if let Some(step) = visited.get(&next) {
						// we've been there, so there is a cycle
						// cut off the extra tail
						acc.drain(..step);
						// there might be more cycles in this cluster,
						// but for they are ignored, the graph shows
						// the entire strongly connected cluster.
					}
					let subsystem_name = &graph[node_idx].to_string();
					let message_name = &graph[edge.id()].to_token_stream().to_string();
					acc.push(format!("{subsystem_name} ~~{{{message_name:?}}}~~> "));
					node_idx = next;
					if next == first {
						break
					}
				} else {
					eprintln!("cycle({print_idx:03}={cycle_tag}): Missing connection in hypothesized cycle after {step} steps, this is a bug 🐛");
					break
				}
			}
			let acc = String::from_iter(acc);
			println!("cycle({print_idx:03}={cycle_tag}): {acc} *");
		}

		cycles
	}

	/// Render a graphviz (aka dot graph) to a file.
	///
	/// Cycles are annotated with the lower
	#[cfg(feature = "graph")]
	pub(crate) fn graphviz(self, dest: &mut impl std::io::Write) -> std::io::Result<()> {
		use petgraph::visit::{EdgeRef, IntoNodeReferences};
		// only write the grap content
		let config = &[
			dot::Config::GraphContentOnly,
			dot::Config::EdgeNoLabel,
			dot::Config::NodeNoLabel,
		][..];

		let Self { mut graph, unsent_messages, unconsumed_messages, cycles } = self;

		// the greek alphabet, lowercase
		let greek_alphabet = greek_alphabet();

		const COLOR_SCHEME_N: usize = 10; // rdylgn10

		// Adding more than 10, is _definitely_ too much visual clutter in the graph.
		const UPPER_BOUND: usize = 10;

		assert!(UPPER_BOUND <= GREEK_ALPHABET_SIZE);
		assert!(UPPER_BOUND <= COLOR_SCHEME_N);

		let n = cycles.len();
		let n = if n > UPPER_BOUND {
			eprintln!("Too many cycles {n}, only annotating the first {UPPER_BOUND} cycles");
			UPPER_BOUND
		} else {
			n
		};

		// restructure for lookups
		let mut cycles_lut = HashMap::<NodeIndex, HashSet<char>>::with_capacity(n);
		// lookup the color index (which is equiv to the index in the cycle set vector _plus one_)
		// based on the cycle_tag (the greek char)
		let mut color_lut = HashMap::<char, usize>::with_capacity(COLOR_SCHEME_N);
		for (cycle_idx, cycle) in cycles.into_iter().take(UPPER_BOUND).enumerate() {
			for node_idx in cycle {
				let _ = cycles_lut.entry(node_idx).or_default().insert(greek_alphabet[cycle_idx]);
			}
			color_lut.insert(greek_alphabet[cycle_idx], cycle_idx + 1);
		}
		let color_lut = &color_lut;

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
		let unsent_node_label = r#"label="✨",fillcolor=black,shape=doublecircle,style=filled,fontname="NotoColorEmoji""#;
		let unconsumed_node_label = r#"label="💀",fillcolor=black,shape=doublecircle,style=filled,fontname="NotoColorEmoji""#;
		let edge_attr = |_graph: &Graph<Ident, Path>,
		                 edge: <&Graph<Ident, Path> as IntoEdgeReferences>::EdgeRef|
		 -> String {
			let source = edge.source();
			let sink = edge.target();

			// use the intersection only, that's the set of cycles the edge is part of
			if let Some(edge_intersecting_cycle_tags) =
				cycles_lut.get(&source).and_then(|source_set| {
					cycles_lut.get(&sink).and_then(move |sink_set| {
						let intersection =
							HashSet::<_, RandomState>::from_iter(source_set.intersection(sink_set));
						if intersection.is_empty() {
							None
						} else {
							Some(intersection)
						}
					})
				}) {
				format!(
					r#"fontcolor="red",xlabel=<{}>,fontcolor="black",label="{}""#,
					cycle_tags_to_annotation(edge_intersecting_cycle_tags, color_lut),
					edge.weight().get_ident().expect("Must have a trailing identifier. qed")
				)
			} else {
				format!(
					r#"label="{}""#,
					edge.weight().get_ident().expect("Must have a trailing identifier. qed")
				)
			}
		};
		let node_attr =
			|_graph: &Graph<Ident, Path>,
			 (node_index, subsystem_name): <&Graph<Ident, Path> as IntoNodeReferences>::NodeRef|
			 -> String {
				if node_index == unsent_idx {
					unsent_node_label.to_owned().clone()
				} else if node_index == unconsumed_idx {
					unconsumed_node_label.to_owned().clone()
				} else if let Some(cycle_tags) = cycles_lut.get(&node_index) {
					format!(
						r#"fontcolor="red",xlabel=<{}>,fontcolor="black",label="{}""#,
						cycle_tags_to_annotation(cycle_tags, color_lut),
						subsystem_name
					)
				} else {
					format!(r#"label="{}""#, subsystem_name)
				}
			};
		let dot = Dot::with_attr_getters(
			&graph, config, &edge_attr, // with state, the reference is a trouble maker
			&node_attr,
		);
		dest.write_all(
			format!(
				r#"digraph {{
	node [colorscheme=rdylgn10]
	{:?}
}}"#,
				&dot
			)
			.as_bytes(),
		)?;
		Ok(())
	}
}

fn cycle_tags_to_annotation<'a>(
	cycle_tags: impl IntoIterator<Item = &'a char>,
	color_lut: &HashMap<char, usize>,
) -> String {
	// Must use fully qualified syntax: <https://github.com/rust-lang/rust/issues/48919>
	let cycle_annotation = String::from_iter(itertools::Itertools::intersperse(
		cycle_tags.into_iter().map(|c| {
			let i = color_lut.get(c).copied().unwrap();
			format!(r#"<B><FONT COLOR="/rdylgn10/{i}">{c}</FONT></B>"#)
		}),
		",".to_owned(),
	));
	cycle_annotation
}

const GREEK_ALPHABET_SIZE: usize = 24;

fn greek_alphabet() -> [char; GREEK_ALPHABET_SIZE] {
	let mut alphabet = ['\u{03B1}'; 24];
	alphabet
		.iter_mut()
		.enumerate()
		// closure should never return `None`,
		// but rather safe than sorry
		.for_each(|(i, c)| {
			*c = char::from_u32(*c as u32 + i as u32).unwrap();
		});
	alphabet
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
