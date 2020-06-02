use statement_table::SignedStatement;
use primitives::Hash;
use std::collections::HashMap;
use futures::channel::mpsc::{Sender, Receiver, channel};
use futures::prelude::*;
use futures::future::ready;
use polkadot_network::legacy::GossipService;
use polkadot_network::legacy::gossip::{GossipMessage, GossipStatement};
use overseer::*;

type Jobs = HashMap<Hash, (exit_future::Signal, Sender<SignedStatement>)>;
type Message = StatementGossipSubsystemMessage;

pub struct StatementGossipSubsystem<GS> {
    /// This comes from the legacy networking code, so it is likely to be changed.
    gossip_service: GS,
}

impl<GS> StatementGossipSubsystem<GS> {
    pub fn new(gossip_service: GS) -> Self {
        Self {
            gossip_service
        }
    }
}

impl<GS: GossipService + Clone + Send + Sync + 'static> Subsystem<Message> for StatementGossipSubsystem<GS> {
    fn start(&mut self, mut ctx: SubsystemContext<Message>) -> SpawnedSubsystem {
        let mut jobs = Jobs::new();
        let gossip_service = self.gossip_service.clone();
        let (jobs_to_subsystem_s, mut jobs_to_subsystem_r) = channel(1024);

        SpawnedSubsystem(Box::pin(async move {
            loop {
                match ctx.try_recv().await {
                    Ok(Some(FromOverseer::Signal(OverseerSignal::StartWork(relay_parent)))) => {
                        let (signal, exit) = exit_future::signal();
                        let (subsystem_to_job_s, subsystem_to_job_r) = channel(1024);
    
                        ctx.spawn(statement_gossip_job(
                            gossip_service.clone(), relay_parent, exit, jobs_to_subsystem_s.clone(), subsystem_to_job_r,
                        ).boxed()).await.unwrap();
    
                        jobs.insert(relay_parent, (signal, subsystem_to_job_s));
                    },
                    Ok(Some(FromOverseer::Signal(OverseerSignal::StopWork(relay_parent)))) => {
                        if let Some((signal, _)) = jobs.remove(&relay_parent) {
                            signal.fire().unwrap();
                        } else {
                            println!("Error: No jobs for {:?} running", relay_parent);
                        }
                    },
                    Ok(Some(FromOverseer::Communication { msg: Message::ToGossip { relay_parent, statement }})) => {
                        if let Some((_, sender)) = jobs.get_mut(&relay_parent) {
                            sender.try_send(statement).unwrap();
                        }
                    },
                    Ok(Some(FromOverseer::Communication { msg: Message::Received { .. }})) => {},
                    Ok(Some(FromOverseer::Signal(OverseerSignal::Conclude))) | Ok(None) | Err(_) => return,
                }

                if let Some(msg) = jobs_to_subsystem_r.next().await {
                    let _ = ctx.send_msg(AllMessages::StatementGossip(msg)).await;
                }
    
            }
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
    mut overseer_sender: Sender<StatementGossipSubsystemMessage>,
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
            overseer_sender.try_send(StatementGossipSubsystemMessage::Received { relay_parent, statement: statement.signed_statement }).unwrap();
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
