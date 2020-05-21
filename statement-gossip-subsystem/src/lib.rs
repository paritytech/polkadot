use statement_table::SignedStatement;
use primitives::Hash;
use std::collections::HashMap;
use futures::channel::mpsc::{Sender, Receiver, channel};
use futures::prelude::*;
use futures::future::ready;
use futures::task::{Spawn, SpawnExt};
use polkadot_network::legacy::GossipService;
use polkadot_network::legacy::gossip::{GossipMessage, GossipStatement};

pub enum StatementGossipSubsystemIn {
    StatementToGossip { relay_parent: Hash, statement: SignedStatement }
}

pub enum StatementGossipSubsystemOut {
    StatementReceived { relay_parent: Hash, statement: SignedStatement }
}

pub struct StatementGossipSubsystem<GS: GossipService, SP: Spawn> {
    /// This comes from the legacy networking code, so it is likely to be changed.
    gossip_service: GS,
    /// A map of relay parent hashes to the associated jobs, and a mono-directional channel for communication.
    jobs: HashMap<Hash, (exit_future::Signal, Sender<SignedStatement>)>,
    /// A sender of messages to the overseer. This is not used directly, only inside of jobs.
    overseer_sender: Sender<StatementGossipSubsystemOut>,
    /// A `futures` Spawner such as `sc_service::SpawnTaskHandle`.
    spawner: SP
}

impl<GS: GossipService + Clone + Send + Sync + 'static, SP: Spawn> StatementGossipSubsystem<GS, SP> {
    pub fn new(overseer_sender: Sender<StatementGossipSubsystemOut>, gossip_service: GS, spawner: SP) -> Self {
        Self {
            jobs: HashMap::new(),
            overseer_sender, gossip_service, spawner
        }
    }

    pub fn start_work(&mut self, relay_parent: Hash) {
        let (signal, exit) = exit_future::signal();
        let (statements_to_gossip_sender, statements_to_gossip_receiver) = channel(1);

        self.spawner.spawn(statement_gossip_job(
            self.gossip_service.clone(), relay_parent, exit, self.overseer_sender.clone(), statements_to_gossip_receiver
        )).unwrap();

        self.jobs.insert(relay_parent, (signal, statements_to_gossip_sender));
    }

    pub fn stop_work(&mut self, relay_parent: Hash) {
        if let Some((signal, _)) = self.jobs.remove(&relay_parent) {
            signal.fire().unwrap();
        } else {
            println!("Error: No jobs for {:?} running", relay_parent);
        }
    }

    pub fn send_message(&mut self, message: StatementGossipSubsystemIn) {
        let StatementGossipSubsystemIn::StatementToGossip { relay_parent, statement } = message;
        if let Some((_, sender)) = self.jobs.get_mut(&relay_parent) {
            sender.try_send(statement).unwrap();
        }
    }
}

async fn statement_gossip_job<GS: GossipService>(
    gossip_service: GS,
    // The relay parent that this job is running for.
    relay_parent: Hash,
    // A future that resolves with the associated `exit_future::Signal` is fired.
    exit_future: exit_future::Exit,
    // A cloned sender of messages to the overseer. This channel is shared between all jobs.
    mut overseer_sender: Sender<StatementGossipSubsystemOut>,
    // A receiver of messages from the subsystem. This channel is exclusive to this job.
    subsystem_receiver: Receiver<SignedStatement>,
) {
    println!("Job for {:?} started", relay_parent);

    let mut incoming = gossip_service.gossip_messages_for(relay_parent)
        .filter_map(|(gossip_message, _)| match gossip_message {
            GossipMessage::Statement(statement) => ready(Some(statement)),
            _ => ready(None)
        })
        .for_each(|statement| {
            overseer_sender.try_send(StatementGossipSubsystemOut::StatementReceived { relay_parent, statement: statement.signed_statement }).unwrap();
            ready(())
        })
        .fuse();

    let mut outgoing = subsystem_receiver.for_each(|statement| {
        gossip_service.gossip_message(relay_parent, GossipMessage::Statement(GossipStatement { relay_chain_leaf: relay_parent, signed_statement: statement }));
        ready(())
    });

    futures::select! {
        _ = exit_future.fuse() => {},
        _ = incoming => {},
        _ = outgoing => {},
    }
    println!("Job for {:?} stopped", relay_parent);
}

#[test]
fn subsystem() {
    struct AsyncStdSpawner;

    impl Spawn for AsyncStdSpawner {
        fn spawn_obj(&self, future: futures::task::FutureObj<'static, ()>) -> Result<(), futures::task::SpawnError> {
            async_std::task::spawn(future);
            Ok(())
        }
    }

    let mock = polkadot_network::protocol::tests::MockGossip::default();
    let (sender, _receiver) = channel(1);
    let mut subsys = StatementGossipSubsystem::new(sender, mock, AsyncStdSpawner);
    subsys.start_work(Hash::zero());
    subsys.stop_work(Hash::zero());
}
