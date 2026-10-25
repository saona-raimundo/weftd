# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](http://keepachangelog.com/en/1.0.0/)
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Also, we try to adhere to the [Conventional Commits specification](https://www.conventionalcommits.org/en/v1.0.0/).

## Unreleased

- feat: edit, add, and remove end wrappers live from the UI (*End wrappers* panel) or with `/wrapper end edit|add|remove`; changes apply from the next turn and are saved with the session. Edits and removals of `in_summarize` wrappers also reach summaries; added wrappers start with `in_summarize = false`
- fix: editing a message at an out-of-range index now reports an error instead of failing silently

## [0.1.0] - 2026-09-24

- First working version
