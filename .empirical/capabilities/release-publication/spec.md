# Release Publication Specification

## Purpose

Define the integrity and traceability guarantees for turning accumulated local
work into an official Agentum GitHub release.

## Requirements

### Requirement: Coherent release contents

The project SHALL publish a patch release only from an exact locally verified
commit containing the intended runtime fixes, matching version metadata, and
accurate release notes. Mutable workstation configuration and workflow state
MUST remain outside the release tree.

#### Scenario: Maintainer releases verified accumulated fixes

- **WHEN** the maintainer requests `v0.18.1` from the current dirty checkout
- **THEN** explicit staging includes only intended production and durable records
- **AND** package metadata, lockfile, changelog, commit, and tag agree on `0.18.1`
- **AND** unrelated local files remain untouched and uncommitted

### Requirement: Traceable GitHub publication

The project SHALL publish the verified commit through a non-force main update,
an immutable annotated tag, and a non-draft GitHub release. Workflow state MUST
be inspected and external account restrictions MUST be reported without being
misclassified as source failures.

#### Scenario: GitHub-hosted jobs cannot start for billing reasons

- **GIVEN** local release verification passed against the exact commit
- **WHEN** GitHub marks tag jobs failed before any step due to account billing
- **THEN** Agentum records the exact annotation and does not alter workflows to hide it
- **AND** the authorized source release may still be published manually
- **AND** no unavailable cross-platform assets are claimed
