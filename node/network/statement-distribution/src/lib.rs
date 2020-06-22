use futures::prelude::*;
use sc_network::{Event, PeerId, ReputationChange};
use sc_network_gossip::Network;
use sp_runtime::{traits::Block as BlockT, ConsensusEngineId};
use std::{borrow::Cow, pin::Pin, sync::Arc};

pub struct StatementGossipSubsystem<B> {
    block: std::marker::PhantomData<B>,
}

impl<B: BlockT> Network<B> for StatementGossipSubsystem<B> {
    /// Returns a stream of events representing what happens on the network.
    fn event_stream(&self) -> Pin<Box<dyn Stream<Item = Event> + Send>> {
        unimplemented!()
    }

    /// Adjust the reputation of a node.
    fn report_peer(&self, peer_id: PeerId, reputation: ReputationChange) {
        unimplemented!()
    }

    /// Force-disconnect a peer.
    fn disconnect_peer(&self, who: PeerId) {
        unimplemented!()
    }

    /// Send a notification to a peer.
    fn write_notification(&self, who: PeerId, engine_id: ConsensusEngineId, message: Vec<u8>) {
        unimplemented!()
    }

    /// Registers a notifications protocol.
    ///
    /// See the documentation of [`NetworkService:register_notifications_protocol`] for more information.
    fn register_notifications_protocol(
        &self,
        engine_id: ConsensusEngineId,
        protocol_name: Cow<'static, [u8]>,
    ) {
        unimplemented!()
    }

    /// Notify everyone we're connected to that we have the given block.
    ///
    /// Note: this method isn't strictly related to gossiping and should eventually be moved
    /// somewhere else.
    fn announce(&self, block: B::Hash, associated_data: Vec<u8>) {
        unimplemented!()
    }
}
