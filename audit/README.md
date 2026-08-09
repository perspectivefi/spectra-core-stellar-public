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

The production contract sources in this release correspond to audited fix
revision `b1f0641e790ae36a474ebed4b360b866caebc7bb` after normalization of the
public-release SPDX header additions. Tests, package metadata, build tooling,
CI, and publication documentation are not part of that source-correspondence
statement.

The complete report is included as
[`Certora - Spectra Core Final Report (2).pdf`](Certora%20-%20Spectra%20Core%20Final%20Report%20(2).pdf).
