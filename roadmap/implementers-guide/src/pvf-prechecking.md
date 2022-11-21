# PVF Pre-checking Overview

> ⚠️ This discusses a mechanism that is currently under-development. Follow the progress under [#3211][3211].

## Motivation

Parachains' and parathreads' validation function is described by a wasm module that we refer to as a PVF. Since a PVF is a wasm module the typical way of executing it is to compile it to machine code.

Typically an optimizing compiler consists of algorithms that are able to optimize the resulting machine code heavily. However, while those algorithms perform quite well for a typical wasm code produced by standard toolchains (e.g. rustc/LLVM), those algorithms can be abused to consume a lot of resources. Moreover, since those algorithms are rather complex there is a lot of room for a bug that can crash the compiler.

If compilation of a Parachain Validation Function (PVF) takes too long or uses too much memory, this can leave a node in limbo as to whether a candidate of that parachain is valid or not.

The amount of time that a PVF takes to compile is a subjective resource limit and as such PVFs may be maliciously crafted so that there is e.g. a 50/50 split of validators which can and cannot compile and execute the PVF.

This has the following implications:
- In backing, inclusion may be slow due to backing groups being unable to execute the block
- In approval checking, there may be many no-shows, leading to slow finality
- In disputes, neither side may reach supermajority. Nobody will get slashed and the chain will not be reverted or finalized.

As a result of this issue we need a fairly hard guarantee that the PVFs of registered parachains/threads can be compiled within a reasonable amount of time.

## Solution

The problem is solved by having a pre-checking process which is run when a new validation code is included in the chain. A new PVF can be added in two cases:

- A new parachain or parathread is registered.
- An existing parachain or parathread signalled an upgrade of its validation code.

Before any of those operations finish, the PVF pre-checking vote is initiated. The PVF pre-checking vote is identified by the PVF code hash that is being voted on. If there is already PVF pre-checking process running, then no
new PVF pre-checking vote will be started. Instead, the operation just subscribes to the existing vote.

The pre-checking vote can be concluded either by obtaining a supermajority or if it expires.

Each validator checks the list of PVFs available for voting. The vote is binary, i.e. accept or reject a given PVF. As soon as the supermajority of votes are collected for one of the sides of the vote, the voting is concluded in that direction and the effects of the voting are enacted.

Only validators from the active set can participate in the vote. The set of active validators can change each session. That's why we reset the votes each session. A voting that observed a certain number of sessions will be rejected.

The effects of the PVF accepting depend on the operations requested it:

1. All onboardings subscribed to the approved PVF pre-checking process will get scheduled and after passing 2 session boundaries they will be onboarded.
1. All upgrades subscribed to the approved PVF pre-checking process will get scheduled very similarly to the existing process. Upgrades with pre-checking are really the same process that is just delayed by the time required for pre-checking voting. In case of instant approval the mechanism is exactly the same.

In case PVF pre-checking process was concluded with rejection, then all the operations that are subscribed to the rejected PVF pre-checking process will be processed as follows. That is, onboarding or upgrading will be cancelled.

The logic described above is implemented by the [paras] module.

On the node-side, there is a PVF pre-checking [subsystem][pvf-prechecker-subsystem] that scans the chain for new PVFs via using [runtime APIs][pvf-runtime-api]. Upon finding a new PVF, the subsystem will initiate a PVF pre-checking request and wait for the result. Whenever the result is obtained, the subsystem will use the [runtime API][pvf-runtime-api] to submit a vote for the PVF. The vote is an unsigned transaction. The vote will be distributed via the gossip similarly to a normal transaction. Eventually a block producer will include the vote into the block where it will be handled by the [runtime][paras].

## Pre-checking Summary

Parachains' and parathreads' validation function is described by a wasm module that we refer to as a PVF.

In order to make the PVF usable for candidate validation it has to be registered on-chain.

As part of the registration process, it has to go through pre-checking. Pre-checking is a game of attempting preparation and reporting the results back on-chain.

We define preparation as a process that: validates the consistency of the wasm binary (aka prevalidation) and the compilation of the wasm module into machine code (refered to as artifact).

Besides pre-checking, preparation can also be triggered by execution, since a compiled artifact is needed for the execution. If an artifact already exists, execution will skip preparation. If it does do preparation, execution uses a more lenient timeout than preparation, to avoid the situation where honest validators fail on valid, pre-checked PVFs.

[3211]: https://github.com/paritytech/polkadot/issues/3211
[paras]: runtime/paras.md
[pvf-runtime-api]: runtime-api/pvf-prechecking.md
[pvf-prechecker-subsystem]: node/utility/pvf-prechecker.md
