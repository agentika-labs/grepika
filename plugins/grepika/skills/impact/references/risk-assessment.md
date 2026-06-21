# Risk Assessment Methodology

This reference provides frameworks for evaluating change impact and refactoring risk.

## Risk Severity Matrix

### Severity Levels

| Level | Definition | Criteria |
|-------|------------|----------|
| **Critical** | System-wide breaking change | Public API change, database schema change, auth flow change |
| **High** | Multiple components affected | Shared utility change, core type change, >10 direct references |
| **Medium** | Localized impact | Single module change, 3-10 references, tests cover main paths |
| **Low** | Isolated change | Single file, <3 references, comprehensive test coverage |

### Impact Factors

| Factor | Weight | Questions |
|--------|--------|-----------|
| **Reference count** | High | How many places use this symbol? |
| **Public exposure** | High | Is this in a public API? |
| **Type signature** | Medium | Does the change affect types? |
| **Test coverage** | Medium | Are there tests for all use cases? |
| **Data persistence** | High | Does this touch stored data? |
| **Cross-module** | Medium | Does it cross module boundaries? |

## Dependency Categories

### Direct Dependencies (Must Update)

These references will break immediately if the target changes:

- Import statements
- Direct function calls
- Type references in signatures
- Class extensions
- Interface implementations

### Indirect Dependencies (May Need Changes)

These references might be affected depending on the nature of the change:

- Re-exports through barrel files
- Dynamic access via `obj[key]`
- Reflection/serialization that includes the name
- Configuration files referencing the symbol
- Documentation mentioning the symbol

### Hidden Dependencies (Often Missed)

These are easy to overlook:

- String-based references in configs
- Test fixtures using the symbol
- Mock implementations
- Type assertions (`as SomeType`)
- Conditional logic checking for the symbol

## Refactoring Patterns

### Safe Rename Pattern

```
1. Add alias/wrapper with new name
2. Update references to use new name
3. Mark old name deprecated
4. Remove old name after all updates
```

Risk: Low (backward compatible during transition)

### Type Change Pattern

```
1. Create union type: OldType | NewType
2. Add runtime type guards
3. Update handlers to support both
4. Migrate data to new type
5. Remove old type support
```

Risk: Medium (requires data migration)

### Signature Change Pattern

```
1. Add optional parameters with defaults
2. Create overload for new signature
3. Update callers incrementally
4. Remove old signature overload
```

Risk: Medium (compile-time errors guide fixes)

### Breaking Change Pattern

```
1. Bump major version
2. Update all internal references
3. Document migration path
4. Provide codemod if possible
```

Risk: High (requires coordinated release)

## Test Coverage Assessment

### Coverage Levels

| Level | Definition | Action |
|-------|------------|--------|
| **Full** | All code paths tested | Proceed with confidence |
| **Partial** | Main paths tested | Test edge cases manually |
| **Minimal** | Only happy path | Add tests before refactoring |
| **None** | No tests | Write characterization tests first |

### What to Check

1. **Unit tests** - Direct tests of the symbol
2. **Integration tests** - Tests using the symbol indirectly
3. **E2E tests** - User-facing tests that exercise the code path
4. **Type tests** - Tests that verify type behavior

## Safe Refactoring Checklist

Before making changes:

- [ ] Mapped all direct references
- [ ] Identified indirect/dynamic usages
- [ ] Checked for string-based references
- [ ] Verified test coverage
- [ ] Identified breaking change potential
- [ ] Created rollback plan

During changes:

- [ ] Making incremental commits
- [ ] Running tests after each change
- [ ] Updating types first (compiler guides fixes)
- [ ] Documenting any API changes

After changes:

- [ ] All tests pass
- [ ] No type errors
- [ ] Updated documentation
- [ ] Informed downstream consumers (if public API)

## Common Refactoring Mistakes

1. **Skipping the search phase** - Missing references causes runtime errors
2. **Ignoring test fixtures** - Mocks become outdated
3. **Forgetting re-exports** - Barrel files hide dependencies
4. **Breaking backward compatibility unnecessarily** - Wrapper functions can preserve old API
5. **Not considering data migration** - Stored data with old shape causes issues
