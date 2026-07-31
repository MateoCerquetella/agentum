# Decisions: durable report export

## D-001: Use atomic replacement

Status: Accepted

### Evidence

A failed replacement must leave the previous report readable.

### Options

1. Write directly to the destination.
2. Write a complete temporary report and replace the destination atomically.

### Chosen approach

Choose option 2.

### Trade-offs and risks

The writer needs a same-filesystem temporary destination.

### Verification

Exercise successful export and failed replacement paths.
