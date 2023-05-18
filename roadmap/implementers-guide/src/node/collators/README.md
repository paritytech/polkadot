# Collators

Collators are special nodes which bridge a parachain to the relay chain. They are simultaneously full nodes of the parachain, and at least light clients of the relay chain. Their overall contribution to the system is the generation of Proofs of Validity for parachain candidates.

The **Collation Generation** subsystem triggers collators to produce collations
and then forwards them to **Collator Protocol** to circulate to validators.
