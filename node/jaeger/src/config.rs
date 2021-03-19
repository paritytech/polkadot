// Copyright 2020 Parity Technologies (UK) Ltd.
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

//! Polkadot Jaeger configuration.

/// Configuration for the jaeger tracing.
#[derive(Clone)]
pub struct JaegerConfig {
	pub(crate) node_name: String,
	pub(crate) agent_addr: std::net::SocketAddr,
}

impl std::default::Default for JaegerConfig {
	fn default() -> Self {
		Self {
			node_name: "unknown_".to_owned(),
			agent_addr: "127.0.0.1:6831"
				.parse()
				.expect(r#"Static "127.0.0.1:6831" is a valid socket address string. qed"#),
		}
	}
}

impl JaegerConfig {
	/// Use the builder pattern to construct a configuration.
	pub fn builder() -> JaegerConfigBuilder {
		JaegerConfigBuilder::default()
	}
}

/// Jaeger configuration builder.
#[derive(Default)]
pub struct JaegerConfigBuilder {
	inner: JaegerConfig,
}

impl JaegerConfigBuilder {
	/// Set the name for this node.
	pub fn named<S>(mut self, name: S) -> Self
	where
		S: AsRef<str>,
	{
		self.inner.node_name = name.as_ref().to_owned();
		self
	}

	/// Set the agent address to send the collected spans to.
	pub fn agent<U>(mut self, addr: U) -> Self
	where
		U: Into<std::net::SocketAddr>,
	{
		self.inner.agent_addr = addr.into();
		self
	}

	/// Construct the configuration.
	pub fn build(self) -> JaegerConfig {
		self.inner
	}
}
