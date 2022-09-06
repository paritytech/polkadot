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

use petgraph::{graph::NodeIndex, Graph};
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
	#[cfg_attr(not(feature = "graph"), allow(dead_code))]
	pub(crate) sccs: Vec<Vec<NodeIndex>>,
	/// Messages that are never being sent (and by which subsystem), but are consumed
	/// Maps the message `Path` to the subsystem `Ident` represented by `NodeIndex`.
	#[cfg_attr(not(feature = "graph"), allow(dead_code))]
	pub(crate) unsent_messages: HashMap<&'a Path, (&'a Ident, NodeIndex)>,
	/// Messages being sent (and by which subsystem), but not consumed by any subsystem
	/// Maps the message `Path` to the subsystem `Ident` represented by `NodeIndex`.
	#[cfg_attr(not(feature = "graph"), allow(dead_code))]
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

		let scc = Self::extract_scc(&graph);

		Self { graph, sccs: scc, unsent_messages, unconsumed_messages }
	}

	/// Extract the strongly connected components (`scc`) which each
	/// includes at least one cycle each.
	fn extract_scc(graph: &Graph<Ident, Path>) -> Vec<Vec<NodeIndex>> {
		use petgraph::visit::EdgeRef;

		// there is no guarantee regarding the node indices in the individual sccs
		let sccs = petgraph::algo::kosaraju_scc(&graph);
		let sccs = Vec::from_iter(sccs.into_iter().filter(|scc| {
			match scc.len() {
				1 => {
					// contains sccs of length one,
					// which do not exists, might be an upstream bug?
					let node_idx = scc[0];
					graph
						.edges_directed(node_idx, petgraph::Direction::Outgoing)
						.find(|edge| edge.target() == node_idx)
						.is_some()
				},
				0 => false,
				_n => true,
			}
		}));
		match sccs.len() {
			0 => println!("✅ Found no strongly connected components, hence no cycles exist"),
			1 => println!(
				"⚡ Found 1 strongly connected component which includes at least one cycle"
			),
			n => println!(
				"⚡ Found {n} strongly connected components which includes at least one cycle each"
			),
		}

		let greek_alphabet = greek_alphabet();

		for (scc_idx, scc) in sccs.iter().enumerate() {
			let scc_tag = greek_alphabet.get(scc_idx).copied().unwrap_or('_');
			let mut acc = Vec::with_capacity(scc.len());
			assert!(scc.len() > 0);
			let mut node_idx = scc[0].clone();
			let print_idx = scc_idx + 1;
			// track which ones were visited and which step
			// the step is required to truncate the output
			// which is required to greedily find a cycle in the strongly connected component
			let mut visited = HashMap::new();
			for step in 0..scc.len() {
				if let Some(edge) =
					graph.edges_directed(node_idx, petgraph::Direction::Outgoing).find(|edge| {
						scc.iter().find(|&scc_node_idx| *scc_node_idx == edge.target()).is_some()
					}) {
					let next = edge.target();
					visited.insert(node_idx, step);

					let subsystem_name = &graph[node_idx].to_string();
					let message_name = &graph[edge.id()].to_token_stream().to_string();
					acc.push(format!("{subsystem_name} ~~{{{message_name:?}}}~~> "));
					node_idx = next;

					if let Some(step) = visited.get(&next) {
						// we've been there, so there is a cycle
						// cut off the extra tail
						assert!(acc.len() >= *step);
						acc.drain(..step);
						// there might be more cycles in this cluster,
						// but for they are ignored, the graph shows
						// the entire strongly connected component.
						break
					}
				} else {
					eprintln!("cycle({print_idx:03}) ∈ {scc_tag}: Missing connection in hypothesized cycle after {step} steps, this is a bug 🐛");
					break
				}
			}
			let acc = String::from_iter(acc);
			println!("cycle({print_idx:03}) ∈ {scc_tag}: {acc} *");
		}

		sccs
	}

	/// Render a graphviz (aka dot graph) to a file.
	///
	/// Cycles are annotated with the lower
	#[cfg(feature = "graph")]
	pub(crate) fn graphviz(self, dest: &mut impl std::io::Write) -> std::io::Result<()> {
		use self::graph_helpers::*;
		use petgraph::{
			dot::{self, Dot},
			visit::{EdgeRef, IntoEdgeReferences, IntoNodeReferences},
		};

		// only write the grap content, we want a custom color scheme
		let config = &[
			dot::Config::GraphContentOnly,
			dot::Config::EdgeNoLabel,
			dot::Config::NodeNoLabel,
		][..];

		let Self { mut graph, unsent_messages, unconsumed_messages, sccs } = self;

		// the greek alphabet, lowercase
		let greek_alphabet = greek_alphabet();

		const COLOR_SCHEME_N: usize = 10; // rdylgn10

		// Adding more than 10, is _definitely_ too much visual clutter in the graph.
		const UPPER_BOUND: usize = 10;

		assert!(UPPER_BOUND <= GREEK_ALPHABET_SIZE);
		assert!(UPPER_BOUND <= COLOR_SCHEME_N);

		let n = sccs.len();
		let n = if n > UPPER_BOUND {
			eprintln!("Too many ({n}) strongly connected components, only annotating the first {UPPER_BOUND}");
			UPPER_BOUND
		} else {
			n
		};

		// restructure for lookups
		let mut scc_lut = HashMap::<NodeIndex, HashSet<char>>::with_capacity(n);
		// lookup the color index (which is equiv to the index in the cycle set vector _plus one_)
		// based on the cycle_tag (the greek char)
		let mut color_lut = HashMap::<char, usize>::with_capacity(COLOR_SCHEME_N);
		for (scc_idx, scc) in sccs.into_iter().take(UPPER_BOUND).enumerate() {
			for node_idx in scc {
				let _ = scc_lut.entry(node_idx).or_default().insert(greek_alphabet[scc_idx]);
			}
			color_lut.insert(greek_alphabet[scc_idx], scc_idx + 1);
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

			let message_name =
				edge.weight().get_ident().expect("Must have a trailing identifier. qed");

			// use the intersection only, that's the set of cycles the edge is part of
			if let Some(edge_intersecting_scc_tags) = scc_lut.get(&source).and_then(|source_set| {
				scc_lut.get(&sink).and_then(move |sink_set| {
					let intersection =
						HashSet::<_, RandomState>::from_iter(source_set.intersection(sink_set));
					if intersection.is_empty() {
						None
					} else {
						Some(intersection)
					}
				})
			}) {
				if edge_intersecting_scc_tags.len() != 1 {
					unreachable!("Strongly connected components are disjunct by definition. qed");
				}
				let scc_tag = edge_intersecting_scc_tags.iter().next().unwrap();
				let color = get_color_by_tag(scc_tag, color_lut);
				let scc_tag_str = cycle_tags_to_annotation(edge_intersecting_scc_tags, color_lut);
				format!(
					r#"color="{color}",fontcolor="{color}",xlabel=<{scc_tag_str}>,label="{message_name}""#,
				)
			} else {
				format!(r#"label="{message_name}""#,)
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
				} else if let Some(edge_intersecting_scc_tags) = scc_lut.get(&node_index) {
					if edge_intersecting_scc_tags.len() != 1 {
						unreachable!(
							"Strongly connected components are disjunct by definition. qed"
						);
					};
					let scc_tag = edge_intersecting_scc_tags.iter().next().unwrap();
					let color = get_color_by_tag(scc_tag, color_lut);

					let scc_tag_str =
						cycle_tags_to_annotation(edge_intersecting_scc_tags, color_lut);
					format!(
						r#"color="{color}",fontcolor="{color}",xlabel=<{scc_tag_str}>,label="{subsystem_name}""#,
					)
				} else {
					format!(r#"label="{subsystem_name}""#)
				}
			};
		let dot = Dot::with_attr_getters(
			&graph, config, &edge_attr, // with state, the reference is a trouble maker
			&node_attr,
		);
		dest.write_all(
			format!(
				r#"digraph {{
	node [colorscheme={}]
	{:?}
}}"#,
				color_scheme(),
				&dot
			)
			.as_bytes(),
		)?;
		Ok(())
	}
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

#[cfg(feature = "graph")]
mod graph_helpers {
	use super::HashMap;

	pub(crate) const fn color_scheme() -> &'static str {
		"rdylgn10"
	}

	pub(crate) fn get_color_by_idx(color_idx: usize) -> String {
		let scheme = color_scheme();
		format!("/{scheme}/{color_idx}")
	}

	pub(crate) fn get_color_by_tag(scc_tag: &char, color_lut: &HashMap<char, usize>) -> String {
		get_color_by_idx(color_lut.get(scc_tag).copied().unwrap_or_default())
	}

	/// A node can be member of multiple cycles,
	/// but only of one strongly connected component.
	pub(crate) fn cycle_tags_to_annotation<'a>(
		cycle_tags: impl IntoIterator<Item = &'a char>,
		color_lut: &HashMap<char, usize>,
	) -> String {
		// Must use fully qualified syntax:
		// <https://github.com/rust-lang/rust/issues/48919>
		let cycle_annotation = String::from_iter(itertools::Itertools::intersperse(
			cycle_tags.into_iter().map(|scc_tag| {
				let color = get_color_by_tag(scc_tag, color_lut);
				format!(r#"<B><FONT COLOR="{color}">{scc_tag}</FONT></B>"#)
			}),
			",".to_owned(),
		));
		cycle_annotation
	}
}

#[cfg(test)]
mod tests {
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

	#[test]
	fn kosaraju_scc_check_nodes_cannot_be_part_of_two_clusters() {
		let mut graph = petgraph::graph::DiGraph::<char, &str>::new();

		let a_idx = graph.add_node('A');
		let b_idx = graph.add_node('B');
		let c_idx = graph.add_node('C');
		let d_idx = graph.add_node('D');
		let e_idx = graph.add_node('E');
		let f_idx = graph.add_node('F');

		graph.add_edge(a_idx, b_idx, "10");
		graph.add_edge(b_idx, c_idx, "11");
		graph.add_edge(c_idx, a_idx, "12");

		graph.add_edge(a_idx, d_idx, "20");
		graph.add_edge(d_idx, c_idx, "21");

		graph.add_edge(b_idx, e_idx, "30");
		graph.add_edge(e_idx, c_idx, "31");

		graph.add_edge(c_idx, f_idx, "40");

		let mut sccs = dbg!(petgraph::algo::kosaraju_scc(&graph));

		dbg!(graph);

		sccs.sort_by(|a, b| {
			if a.len() < b.len() {
				std::cmp::Ordering::Greater
			} else {
				std::cmp::Ordering::Less
			}
		});
		assert_eq!(sccs.len(), 2); // `f` and everything else
		assert_eq!(sccs[0].len(), 5); // every node but `f`
	}
}
