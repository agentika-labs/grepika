# Codebase Onboarding Strategies

This reference provides approaches for learning codebases of different types and sizes.

## Codebase Type Patterns

### Monolith Applications

**Characteristics**: Single deployment, shared database, internal modules

**Exploration Strategy**:
1. Find main entry point (server.ts, index.ts, main.py)
2. Trace request flow from routes to handlers to services
3. Identify shared utilities and core abstractions
4. Map database models and relationships

**Key Searches**:
- `app.listen` or `createServer` - Find server startup
- `router` or `routes` - Find API endpoints
- `schema` or `model` - Find data models
- `middleware` - Find request processing

### Microservices

**Characteristics**: Multiple deployments, service boundaries, async communication

**Exploration Strategy**:
1. Map services via docker-compose, kubernetes configs, or package.json workspaces
2. Find API contracts (OpenAPI, GraphQL schemas, protobuf)
3. Identify message queues and event patterns
4. Trace cross-service communication

**Key Searches**:
- `publish` or `emit` - Find event producers
- `subscribe` or `on` - Find event consumers
- `fetch` or `axios` or `grpc` - Find service calls
- `.proto` or `schema.graphql` - Find contracts

### Libraries/SDKs

**Characteristics**: Public API, documentation focus, versioning

**Exploration Strategy**:
1. Read package.json for entry points
2. Find public exports (index.ts, exports field)
3. Understand the core abstraction/pattern
4. Trace from public API to implementation

**Key Searches**:
- `export` in index files - Find public API surface
- `@public` or `@api` - Find documented APIs
- `README` or `docs/` - Find documentation
- `examples/` - Find usage patterns

### CLI Tools

**Characteristics**: Argument parsing, subcommands, stdout/stderr output

**Exploration Strategy**:
1. Find argument parser setup (yargs, commander, clap)
2. Map commands to handlers
3. Understand configuration loading
4. Trace command execution flow

**Key Searches**:
- `command` or `subcommand` - Find CLI commands
- `option` or `flag` or `arg` - Find argument definitions
- `process.exit` - Find exit points
- `console.log` or `stdout` - Find output handling

## Learning Approaches

### Top-Down (Architecture First)

Best for: Large codebases, when you need the big picture

```
1. toc() → understand directory structure
2. stats() → see language distribution
3. search("main entry") → find starting points
4. outline() on core files → see structure
5. Dive into specific areas as needed
```

### Bottom-Up (Feature First)

Best for: When you need to make a specific change

```
1. search("feature keyword") → find related files
2. refs() → trace connections from feature code
3. outline() → understand local structure
4. Expand outward to understand context
```

### Follow-The-Data

Best for: Understanding business logic

```
1. Find data model definitions
2. Trace CRUD operations for each model
3. Map data transformations through layers
4. Identify validation and business rules
```

## Output Templates

### Quick Overview (5 minutes)

```markdown
## [Project Name] Overview

**Type**: [monolith/microservices/library/CLI]
**Languages**: [primary, secondary]
**Size**: ~[X] files, ~[Y]K lines

### Key Directories
- `src/` - [purpose]
- `lib/` - [purpose]
- `tests/` - [purpose]

### Start Here
1. [path] - Entry point
2. [path] - Core abstraction
3. [path] - Main configuration
```

### Deep Dive (30 minutes)

```markdown
## [Project Name] Architecture

### System Overview
[2-3 paragraph description]

### Module Map
| Module | Location | Responsibility | Dependencies |
|--------|----------|----------------|--------------|
| [name] | [path]   | [what it does] | [what it uses] |

### Data Flow
[Description of how data moves through the system]

### Key Patterns
- **[Pattern 1]**: Used in [where], purpose is [why]
- **[Pattern 2]**: Used in [where], purpose is [why]

### Configuration
| Config | Location | Purpose |
|--------|----------|---------|
| [name] | [path]   | [what it configures] |

### Recommended Reading Order
1. **[file]** - Understand [concept] first
2. **[file]** - Then learn about [concept]
3. **[file]** - Finally explore [concept]

### Common Tasks
| Task | Start Here | Key Files |
|------|------------|-----------|
| Add feature | [path] | [files] |
| Fix bug | [path] | [files] |
| Add test | [path] | [files] |
```

## Common Gotchas

1. **Generated code** - Look for `// @generated` or `generated/` directories
2. **Vendored dependencies** - Code in `vendor/` or `third_party/` is external
3. **Legacy code** - `deprecated/` or `legacy/` directories are often dead code
4. **Configuration hell** - Multiple config files can override each other
5. **Magic strings** - Convention-based frameworks (Rails, Django) use naming

## Questions to Ask Early

- What's the deployment model?
- Where is configuration stored?
- How are environment differences handled?
- What's the testing strategy?
- Where are the integration points with external systems?
- What are the known problem areas?
