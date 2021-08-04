# Overseer

The overseer is responsible for these tasks:

1. Setting up, monitoring, and handing failure for overseen subsystems.
1. Providing a "heartbeat" of which relay-parents subsystems should be working on.
1. Acting as a message bus between subsystems.

The hierarchy of subsystems:

```text
+--------------+      +------------------+    +--------------------+
|              |      |                  |---->   Subsystem A      |
| Block Import |      |                  |    +--------------------+
|    Events    |------>                  |    +--------------------+
+--------------+      |                  |---->   Subsystem B      |
                      |   Overseer       |    +--------------------+
+--------------+      |                  |    +--------------------+
|              |      |                  |---->   Subsystem C      |
| Finalization |------>                  |    +--------------------+
|    Events    |      |                  |    +--------------------+
|              |      |                  |---->   Subsystem D      |
+--------------+      +------------------+    +--------------------+

```

The overseer determines work to do based on block import events and block finalization events. It does this by keeping track of the set of relay-parents for which work is currently being done. This is known as the "active leaves" set. It determines an initial set of active leaves on startup based on the data on-disk, and uses events about blockchain import to update the active leaves. Updates lead to [`OverseerSignal`](../types/overseer-protocol.md#overseer-signal)`::ActiveLeavesUpdate` being sent according to new relay-parents, as well as relay-parents to stop considering. Block import events inform the overseer of leaves that no longer need to be built on, now that they have children, and inform us to begin building on those children. Block finalization events inform us when we can stop focusing on blocks that appear to have been orphaned.

The overseer is also responsible for tracking the freshness of active leaves. Leaves are fresh when they're encountered for the first time, and stale when they're encountered for subsequent times. This can occur after chain reversions or when the fork-choice rule abandons some chain. This distinction is used to manage **Reversion Safety**. Consensus messages are often localized to a specific relay-parent, and it is often a misbehavior to equivocate or sign two conflicting messages. When reverting the chain, we may begin work on a leaf that subsystems have already signed messages for. Subsystems which need to account for reversion safety should avoid performing work on stale leaves.

The overseer's logic can be described with these functions:

## On Startup

* Start all subsystems
* Determine all blocks of the blockchain that should be built on. This should typically be the head of the best fork of the chain we are aware of. Sometimes add recent forks as well.
* Send an `OverseerSignal::ActiveLeavesUpdate` to all subsystems with `activated` containing each of these blocks.
* Begin listening for block import and finality events

## On Block Import Event

* Apply the block import event to the active leaves. A new block should lead to its addition to the active leaves set and its parent being deactivated.
* Mark any stale leaves as stale. The overseer should track all leaves it activates to determine whether leaves are fresh or stale.
* Send an `OverseerSignal::ActiveLeavesUpdate` message to all subsystems containing all activated and deactivated leaves.
* Ensure all `ActiveLeavesUpdate` messages are flushed before resuming activity as a message router.

> TODO: in the future, we may want to avoid building on too many sibling blocks at once. the notion of a "preferred head" among many competing sibling blocks would imply changes in our "active leaves" update rules here

## On Finalization Event

* Note the height `h` of the newly finalized block `B`.
* Prune all leaves from the active leaves which have height `<= h` and are not `B`.
* Issue `OverseerSignal::ActiveLeavesUpdate` containing all deactivated leaves.

## On Subsystem Failure

Subsystems are essential tasks meant to run as long as the node does. Subsystems can spawn ephemeral work in the form of jobs, but the subsystems themselves should not go down. If a subsystem goes down, it will be because of a critical error that should take the entire node down as well.

## Communication Between Subsystems

When a subsystem wants to communicate with another subsystem, or, more typically, a job within a subsystem wants to communicate with its counterpart under another subsystem, that communication must happen via the overseer. Consider this example where a job on subsystem A wants to send a message to its counterpart under subsystem B. This is a realistic scenario, where you can imagine that both jobs correspond to work under the same relay-parent.

```text
     +--------+                                                           +--------+
     |        |                                                           |        |
     |Job A-1 | (sends message)                       (receives message)  |Job B-1 |
     |        |                                                           |        |
     +----|---+                                                           +----^---+
          |                  +------------------------------+                  ^
          v                  |                              |                  |
+---------v---------+        |                              |        +---------|---------+
|                   |        |                              |        |                   |
| Subsystem A       |        |       Overseer / Message     |        | Subsystem B       |
|                   -------->>                  Bus         -------->>                   |
|                   |        |                              |        |                   |
+-------------------+        |                              |        +-------------------+
                             |                              |
                             +------------------------------+
```

First, the subsystem that spawned a job is responsible for handling the first step of the communication. The overseer is not aware of the hierarchy of tasks within any given subsystem and is only responsible for subsystem-to-subsystem communication. So the sending subsystem must pass on the message via the overseer to the receiving subsystem, in such a way that the receiving subsystem can further address the communication to one of its internal tasks, if necessary.

This communication prevents a certain class of race conditions. When the Overseer determines that it is time for subsystems to begin working on top of a particular relay-parent, it will dispatch a `ActiveLeavesUpdate` message to all subsystems to do so, and those messages will be handled asynchronously by those subsystems. Some subsystems will receive those messsages before others, and it is important that a message sent by subsystem A after receiving `ActiveLeavesUpdate` message will arrive at subsystem B after its `ActiveLeavesUpdate` message. If subsystem A maintained an independent channel with subsystem B to communicate, it would be possible for subsystem B to handle the side message before the `ActiveLeavesUpdate` message, but it wouldn't have any logical course of action to take with the side message - leading to it being discarded or improperly handled. Well-architectured state machines should have a single source of inputs, so that is what we do here.

One exception is reasonable to make for responses to requests. A request should be made via the overseer in order to ensure that it arrives after any relevant `ActiveLeavesUpdate` message. A subsystem issuing a request as a result of a `ActiveLeavesUpdate` message can safely receive the response via a side-channel for two reasons:

1. It's impossible for a request to be answered before it arrives, it is provable that any response to a request obeys the same ordering constraint.
1. The request was sent as a result of handling a `ActiveLeavesUpdate` message. Then there is no possible future in which the `ActiveLeavesUpdate` message has not been handled upon the receipt of the response.

So as a single exception to the rule that all communication must happen via the overseer we allow the receipt of responses to requests via a side-channel, which may be established for that purpose. This simplifies any cases where the outside world desires to make a request to a subsystem, as the outside world can then establish a side-channel to receive the response on.

It's important to note that the overseer is not aware of the internals of subsystems, and this extends to the jobs that they spawn. The overseer isn't aware of the existence or definition of those jobs, and is only aware of the outer subsystems with which it interacts. This gives subsystem implementations leeway to define internal jobs as they see fit, and to wrap a more complex hierarchy of state machines than having a single layer of jobs for relay-parent-based work. Likewise, subsystems aren't required to spawn jobs. Certain types of subsystems, such as those for shared storage or networking resources, won't perform block-based work but would still benefit from being on the Overseer's message bus. These subsystems can just ignore the overseer's signals for block-based work.

Furthermore, the protocols by which subsystems communicate with each other should be well-defined irrespective of the implementation of the subsystem. In other words, their interface should be distinct from their implementation. This will prevent subsystems from accessing aspects of each other that are beyond the scope of the communication boundary.

## On shutdown

Send an `OverseerSignal::Conclude` message to each subsystem and wait some time for them to conclude before hard-exiting.
