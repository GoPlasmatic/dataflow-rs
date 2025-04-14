# Source Functions

Source functions are responsible for ingesting events from external systems and converting them into structured messages for the workflow engine.

## Overview

Source functions convert external events or payloads into structured messages ready for processing by the system. They ensure that incoming data is properly formatted and normalized to match the expected schema of the engine.

## Categories of Source Functions

- **HTTP Source Functions**:
  - Listen for inbound HTTP requests, such as RESTful endpoints or webhook callbacks.
  - Convert the request data into a well-structured message.

- **WebSocket Source Functions**:
  - Maintain persistent connections with clients or external services.
  - Ingest continuous data streams and transform them into messages.

- **File-Based Source Functions**:
  - Monitor directories for file changes.
  - Trigger messages based on file creations, modifications, or deletions.

- **Timer/Cron Source Functions**:
  - Generate messages based on scheduled events.
  - Ideal for periodic polling, cleanup tasks, or scheduled automation.

- **Message Broker Source Functions**:
  - Integrate with messaging systems such as Kafka, RabbitMQ, or Redis Streams.
  - Consume events and translate them into messages for further processing.

## Implementation Considerations

- Ensure proper error handling and retries in case the external source fails.
- Normalize incoming payloads to maintain consistency across diverse data sources.
- Implement asynchronous processing when necessary to handle high throughput scenarios. 