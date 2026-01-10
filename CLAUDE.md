# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Dataflow-rs is a lightweight, rule-driven workflow engine for building data processing pipelines and nanoservices in Rust. It provides an async-first execution model with pre-compiled JSONLogic for high performance.

### Core Architecture

- **Engine**: Central async component that processes messages through workflows sequentially
- **Workflow**: Collection of tasks with JSONLogic conditions (can only access metadata fields)
- **Task**: Individual processing units that implement `AsyncFunctionHandler` trait
- **Message**: Data structure containing `data`, `payload`, `metadata`, `temp_data`, audit trail, and errors
- **Built-in Functions**: Data mapping/transformation and validation

### Key Design Patterns

- **Sequential Workflow Processing**: Workflows execute sequentially to allow dependencies between workflows
- **Pre-compiled JSONLogic**: All logic expressions compiled at startup for zero runtime overhead
- **Retry Mechanisms**: Configurable retry policies with exponential backoff for transient failures
- **Audit Trails**: Automatic change tracking for debugging and monitoring

## Development Commands

### Build and Test
```bash
# Build the project
cargo build

# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run examples
cargo run --example benchmark            # Performance benchmark
cargo run --example custom_function      # Custom function implementation
cargo run --example complete_workflow    # Complete workflow example
```

### Code Quality
The project uses standard Rust tooling:
```bash
# Format code
cargo fmt

# Lint code
cargo clippy

# Check without building
cargo check
```

### Release Process
The project uses GitHub Actions for automated releases via `cargo-release` when pushing to main branch.

## Code Structure

### Core Engine (`src/engine/`)
- `mod.rs`: Main Engine implementation with async message processing
- `compiler.rs`: JSONLogic compilation and caching
- `executor.rs`: Internal function execution
- `workflow_executor.rs`: Workflow orchestration
- `task_executor.rs`: Task execution
- `message.rs`: Message structure with data, metadata, and audit trail
- `workflow.rs`: Workflow definition and validation
- `task.rs`: Task structure and Function definition
- `error.rs`: Comprehensive error types (DataflowError, ErrorInfo)
- `utils.rs`: Helper utilities for data manipulation

### Built-in Functions (`src/engine/functions/`)
- `map.rs`: Data transformation using JSONLogic (supports array notation)
- `validation.rs`: Rule-based validation with custom error messages
- `mod.rs`: Registration and management of built-in functions

### Key Implementation Details

- **Workflow Conditions**: Can ONLY access `metadata` fields, not `data` fields
- **Task Dependencies**: Tasks within workflows execute sequentially, allowing later tasks to depend on earlier results
- **Error Handling**: Workflows can continue processing despite individual task failures when `continue_on_error` is enabled
- **Custom Functions**: Implement `AsyncFunctionHandler` trait with async `execute()` returning `Result<(usize, Vec<Change>)>`
- **Structure Preservation**: DataLogic instances are configured with `with_preserve_structure()` to maintain object structure in JSONLogic operations
- **Async-First**: Engine uses async/await for all operations with tokio runtime support

### Testing Patterns

The test suite demonstrates:
- Custom async function handler implementation
- Workflow engine integration testing
- Message processing verification
- Data mapping and transformation patterns

When extending the engine:
1. Implement `AsyncFunctionHandler` for custom tasks
2. Register functions with engine constructor or `register_task_function()`
3. Use `Change` structs to track modifications for audit trails
4. Handle errors appropriately and return proper status codes