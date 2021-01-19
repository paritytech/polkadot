# Type Definitions

This section of the guide provides type definitions of various categories.

## V1 Overview

These data types are defined in `polkadot/primitives/src/v1.rs`:

```dot process
digraph {
    node [shape = plain]

    CandidateDescriptor [label = <
        <table>
            <tr><td border="0" colspan="2">CandidateDescriptor&lt;H = Hash&gt;</td></tr>
            <tr><td>para_id</td><td port="para_id">Id</td></tr>
            <tr><td>relay_parent</td><td port="relay_parent">H</td></tr>
            <tr><td>collator</td><td port="collator">CollatorId</td></tr>
            <tr><td>persisted_validation_data_hash</td><td port="persisted_validation_data_hash">Hash</td></tr>
            <tr><td>pov_hash</td><td port="pov_hash">Hash</td></tr>
            <tr><td>erasure_root</td><td port="erasure_root">Hash</td></tr>
            <tr><td>signature</td><td port="signature">CollatorSignature</td></tr>
        </table>
    >]

    CandidateReceipt [label = <
        <table>
            <tr><td border="0" colspan="2">CandidateReceipt&lt;H = Hash&gt;</td></tr>
            <tr><td>descriptor</td><td port="descriptor">CandidateDescriptor&lt;H&gt;</td></tr>
            <tr><td>commitments_hash</td><td port="commitments_hash">Hash</td></tr>
        </table>
    >]

    CandidateReceipt:descriptor -> CandidateDescriptor

    FullCandidateReceipt [label = <
        <table>
            <tr><td border="0" colspan="2">FullCandidateReceipt&lt;H = Hash, N = BlockNumber&gt;</td></tr>
            <tr><td>inner</td><td port="inner">CandidateReceipt&lt;H&gt;</td></tr>
            <tr><td>validation_data</td><td port="validation_data">ValidationData&lt;N&gt;</td></tr>
        </table>
    >]

    FullCandidateReceipt:inner -> CandidateReceipt
    FullCandidateReceipt:validation_data -> ValidationData

    CommittedCandidateReceipt [label = <
        <table>
            <tr><td border="0" colspan="2">CommittedCandidateReceipt&lt;H = Hash&gt;</td></tr>
            <tr><td>descriptor</td><td port="descriptor">CandidateDescriptor&lt;H&gt;</td></tr>
            <tr><td>commitments</td><td port="commitments">CandidateCommitments</td></tr>
        </table>
    >]

    CommittedCandidateReceipt:descriptor -> CandidateDescriptor
    CommittedCandidateReceipt:commitments -> CandidateCommitments

    ValidationData [label = <
        <table>
            <tr><td border="0" colspan="2">ValidationData&lt;N = BlockNumber&gt;</td></tr>
            <tr><td>persisted</td><td port="persisted">PersistedValidationData&lt;N&gt;</td></tr>
            <tr><td>transient</td><td port="transient">TransientValidationData&lt;N&gt;</td></tr>
        </table>
    >]

    ValidationData:persisted -> PersistedValidationData
    ValidationData:transient -> TransientValidationData

    PersistedValidationData [label = <
        <table>
            <tr><td border="0" colspan="2">PersistedValidationData&lt;N = BlockNumber&gt;</td></tr>
            <tr><td>parent_head</td><td port="parent_head">HeadData</td></tr>
            <tr><td>block_number</td><td port="block_number">N</td></tr>
            <tr><td>relay_storage_root</td><td port="relay_storage_root">Hash</td></tr>
            <tr><td>hrmp_mqc_heads</td><td port="hrmp_mqc_heads">Vec&lt;(Id, Hash)&gt;</td></tr>
            <tr><td>dmq_mqc_head</td><td port="dmq_mqc_head">Hash</td></tr>
            <tr><td>max_pov_size</td><td port="max_pov_size">u32</td></tr>
        </table>
    >]
}
```
