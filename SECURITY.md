# Security Policy

## Reporting a Vulnerability

Please report security vulnerabilities privately using [GitHub Security Advisories](https://github.com/AZX-PBC-OSS/opportune/security/advisories/new) rather than opening a public issue.

Include as much detail as possible to help reproduce and assess the issue:

- Affected package(s) and version (`opportune`, `opportune-json`, `opportune-adapters`, Python wheel, or npm package)
- Minimal reproduction input (JSON request, provider payload, or code snippet)
- Observed behavior and security impact (for example, panic, resource exhaustion, or data exposure)

We will acknowledge receipt and coordinate a fix and disclosure timeline through the advisory.

## Security-Relevant Usage Notes

- **Operator diagnostics are not public availability data.** `Response.warnings`, `Response.excluded`, `OfferEvent.eligible`/`assignment`/`reserve`, and `SlotCheck.reasons`/`warnings` contain internal host IDs, rule IDs, and calendar details. On public booking endpoints, force `explain` unset and project answers through `public_response` / `public_slot_check` (Python) or `publicResponse` / `publicSlotCheck` (TypeScript).
- **Enforce HTTP request body limits at the proxy or middleware boundary.** `Engine` limits bound parsed scheduling structures, not raw HTTP payload decoding.
- **Keep confirmation atomic.** `check_slot` is pure and stateless; pair it with a database transaction or unique constraint when committing a booking.
