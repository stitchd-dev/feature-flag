# Multi-Tenancy Model

```mermaid
graph TD
    T[Tenant] --> P[Project]
    P --> E[Environment]
    E --> SDK[SDK Keys]
    P --> FF[Feature Flag Definitions]
    P --> V[Variant Configurations]
    E --> R[Rules]
    E --> S[Segments]
    E --> EX[Experiments]
```

## Scoping

- **Project level:** Feature Flag definitions and Variant configurations are shared across environments.
- **Environment level:** Rules, Segments, Experiments, and SDK Keys are environment-scoped.

## SDK Key Constraints

- Each environment must have at least one active SDK key at all times.
- Keys are scoped to a single `project + environment` pair.
- Project Admins manage key creation and revocation.
