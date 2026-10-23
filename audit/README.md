# Certora Security Review

Certora reviewed Spectra Core from June 29, 2026 through July 20, 2026. The
review began at source revision
`684288baa095ad182c0fc0b8a2ce3a85871f055c` and verified fixes at audited
revision `b1f0641e790ae36a474ebed4b360b866caebc7bb`.

The audited scope comprised 46 production Rust source files across the Blend
vault wrapper, factory, limit-order engine, principal token, registry, router,
shared interfaces and utilities, Spectra vault wrapper, math library, and yield
token. External systems integrated through contract-client traits were outside
the audit scope. The report contains the authoritative file-by-file scope.

Certora reported 32 findings: one high, six medium, eleven low, and fourteen
informational. Thirty-one were confirmed and 25 were fixed. The remaining
confirmed findings are marked "Acknowledged" in the report. Consult each
finding for its exact status, context, and remediation.

The production contract sources in the original public release
(`28d955f278162f596bc763fc444272e2a23ff34a`) correspond to audited fix revision
`b1f0641e790ae36a474ebed4b360b866caebc7bb` after normalization of the
public-release SPDX header additions. Tests, package metadata, build tooling,
CI, and publication documentation are not part of that source-correspondence
statement.

## Subsequent contract changes

The current source tree includes the following changes after the audited
revision. These changes are not covered by the included Certora report.

| Component | Change |
| --- | --- |
| Limit-order engine | Round maker proceeds and fees up for every order type at the pricing step, preventing dust-fill underpayment from those calculations. |
| Spectra vault wrapper base | Expose the stored share decimals through `share_decimals`. |
| Blend vault wrapper | Use the base share-decimal helper for `decimals`; Blend's stored share and asset decimals coincide. |

Dependency updates are recorded in `Cargo.lock`. Source correspondence does not
establish identical compiled WASM or the version of a deployed contract.

The complete report is included as
[`Certora - Spectra Core Final Report (2).pdf`](Certora%20-%20Spectra%20Core%20Final%20Report%20(2).pdf).
