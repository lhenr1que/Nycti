# Configuration

Configuration owns CLEA's configuration contracts. Every supported
configuration format must have an explicit schema and version so consumers can
validate compatibility and evolve safely.

This component defines configuration boundaries shared by clients and services;
it does not own their presentation or runtime policy.
