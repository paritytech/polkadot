use statement_table::SignedStatement;
use primitives::Hash;
use std::collections::HashMap;
use futures::channel::mpsc::{Sender, Receiver, channel};
use futures::prelude::*;
use futures::future::ready;
use futures::task::{Spawn, SpawnExt};
use polkadot_network::legacy::GossipService;
use polkadot_network::legacy::gossip::{GossipMessage, GossipStatement};
use overseer::*;

#[derive(Debug, Clone)]
pub enum StatementGossipMessage {
    ToGossip { relay_parent: Hash, statement: SignedStatement },
    Received { relay_parent: Hash, statement: SignedStatement }
}

type Jobs = HashMap<Hash, (exit_future::Signal, Sender<SignedStatement>)>;

#[derive(Debug, Clone)]
enum Message {
    StartWork(Hash),
    StopWork(Hash),
    StatementGossip(StatementGossipMessage),
}

pub struct StatementGossipSubsystem<GS, SP> {
    /// This comes from the legacy networking code, so it is likely to be changed.
    gossip_service: GS,
    /// A `futures` Spawner such as `sc_service::SpawnTaskHandle`.
    spawner: SP
}

impl<GS, SP> StatementGossipSubsystem<GS, SP> {
    pub fn new(gossip_service: GS, spawner: SP) -> Self {
        Self {
            gossip_service, spawner
        }
    }
}

impl<GS: GossipService + Clone + Send + Sync + 'static, SP: Spawn + Clone + 'static> Subsystem<Message> for StatementGossipSubsystem<GS, SP> {
    fn start(&mut self, rx: Receiver<Message>, mut tx: Sender<OverseerMessage<Message>>) -> SubsystemJob {
        let mut jobs = Jobs::new();
        let gossip_service = self.gossip_service.clone();
        let spawner = self.spawner.clone();
        let (jobs_to_subsystem_s, jobs_to_subsystem_r) = channel(1024);

        let incoming = rx.for_each(move |message| {
            match message {
                Message::StartWork(relay_parent) => {
                    let (signal, exit) = exit_future::signal();
                    let (subsystem_to_job_s, subsystem_to_job_r) = channel(1024);

                    spawner.spawn(statement_gossip_job(
                        gossip_service.clone(), relay_parent, exit, jobs_to_subsystem_s.clone(), subsystem_to_job_r,
                    )).unwrap();

                    jobs.insert(relay_parent, (signal, subsystem_to_job_s));
                },
                Message::StopWork(relay_parent) => {
                    if let Some((signal, _)) = jobs.remove(&relay_parent) {
                        signal.fire().unwrap();
                    } else {
                        println!("Error: No jobs for {:?} running", relay_parent);
                    }
                },
                Message::StatementGossip(StatementGossipMessage::ToGossip { relay_parent, statement }) => {
                    if let Some((_, sender)) = jobs.get_mut(&relay_parent) {
                        sender.try_send(statement).unwrap();
                    }
                },
                _ => {}
            }

            futures::future::ready(())
        });
        
        let outgoing = jobs_to_subsystem_r.for_each(move |message| {
            tx.try_send(OverseerMessage::SubsystemMessage { to: None, msg: Message::StatementGossip(message) });

            futures::future::ready(())
        });

		SubsystemJob(Box::pin(async move {
            futures::join!(incoming, outgoing);
            Ok(())
        }))
	}
}

async fn statement_gossip_job<GS: GossipService>(
    gossip_service: GS,
    // The relay parent that this job is running for.
    relay_parent: Hash,
    // A future that resolves with the associated `exit_future::Signal` is fired.
    exit_future: exit_future::Exit,
    // A cloned sender of messages to the overseer. This channel is shared between all jobs.
    mut overseer_sender: Sender<StatementGossipMessage>,
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
            overseer_sender.try_send(StatementGossipMessage::Received { relay_parent, statement: statement.signed_statement }).unwrap();
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
    env_logger::init();

    #[derive(Clone)]
    struct AsyncStdSpawner;

    impl Spawn for AsyncStdSpawner {
        fn spawn_obj(&self, future: futures::task::FutureObj<'static, ()>) -> Result<(), futures::task::SpawnError> {
            async_std::task::spawn(future);
            Ok(())
        }
    }

    let mock = polkadot_network::protocol::tests::MockGossip::default();

    futures::executor::block_on(async {
		let subsystems: Vec<Box<dyn Subsystem<Message>>> = vec![
			Box::new(StatementGossipSubsystem::new(mock, AsyncStdSpawner)),
		];

		let overseer = Overseer::new(subsystems);
		overseer.run().await;
	});
}
