## Purpose

This capability documents durable, deterministic report export behavior.

## ADDED Requirements

### Requirement: A report can be exported

The system MUST write a durable report through a deterministic export boundary.

#### Scenario: Observable export result

- **WHEN** a user requests a report export
- **THEN** the completed report is visible at the selected destination

### Requirement: Failed replacement preserves prior output

The system MUST leave an existing report unchanged when replacement fails.

#### Scenario: Destination replacement fails

- **WHEN** the destination cannot be replaced atomically
- **THEN** the previous report remains readable and the failure is reported
