# Incurs domain context

| Field | Value |
| --- | --- |
| Status | Active terminology registry |
| Audience | Incurs maintainers and adapter authors |
| Outcome | Classify system concepts without conflating prompts, tools, and human documentation |
| Writing profile | Technical and developer documentation guided by UWDS 1.0 |

## Controlling distinction

Classify a system concept by its consumer and effect, not by its encoding.
Text can represent executable code, structured data, human documentation, or a
model prompt. A string is not a prompt merely because a model can read it.

Every module, field, and generated artifact has one primary kind. When one
value has responsibilities from multiple kinds, split it into linked values
that share a stable Capability identity.

```text
Capability
  |
  +-- Tool Contract -> Tool Binding -> Tool Runtime -> Tool Result
  |
  +-- Prompt Guide -> Prompt Compiler -> Prompt Artifact -> Publisher
  |
  `-- Documentation Source -> Documentation Artifact
```

## Preferred terms

### Capability

A target-neutral description of an action that the system can perform. A
Capability owns stable identity, input and output meaning, effects, and
permissions. It does not own a model prompt, host binding, or executable
handler.

Examples include a command's canonical identity, field schemas, effect
classification, and result shape.

### Prompt Guide

Target-neutral guidance that tells a model how, when, or why to use one or more
Capabilities. A Prompt Guide can include instructions, selection heuristics,
warnings, sequencing rules, and natural-language demonstrations.

A Prompt Guide can reference a Capability by identity. It cannot contain an
executable handler or become required for direct Capability invocation.

### Prompt Compiler

A module that lowers Prompt Guides and Capability facts into a target-specific
Prompt Artifact. Prompt compilation is deterministic and does not execute
Tools.

### Prompt Artifact

Compiled model-directed content consumed as context or instructions. Examples
include `SKILL.md`, `AGENTS.md`, system prompts, MCP prompts, and `--llms`
output.

Changing a Prompt Artifact can change model behavior. It cannot change a Tool
Contract, Tool policy, or Tool identity.

### Tool Contract

The machine-readable agreement for invoking a Capability. A Tool Contract owns
the exposed name, input and output schemas, error shape, effect annotations,
and policy-relevant metadata.

A Tool Contract can include a neutral summary needed for discovery. It cannot
include model instructions, selection heuristics, or few-shot examples.

### Tool Binding

A host- or protocol-specific adapter that exposes a Tool Contract. Examples
include an MCP tool, CLI command, HTTP operation, and Code Mode method.

The protocol does not determine the kind. MCP tools are Tool Bindings, while
MCP prompts are Prompt Artifacts.

### Tool Runtime

The executable module that resolves Tool Contracts, invokes Capability
handlers, enforces policy, handles cancellation, and emits ordered events. A
Tool Runtime does not compile or publish prompts.

### Tool Result

Structured data produced by Tool execution, including successful values,
errors, artifacts, progress events, and presentation metadata. Text inside a
Tool Result remains data. It becomes model instruction only when a Prompt
module explicitly incorporates it.

### Documentation Artifact

Human-facing explanatory material that does not direct model behavior or
define machine invocation. Examples include a README, reference documentation,
and CLI help intended for people.

When the same source material is emitted for both people and models, compile
separate Documentation and Prompt Artifacts so each has an explicit consumer.

### Adapter

A module that converts one representation into another at a seam. Adapter
names state both their target and kind, such as `McpToolAdapter` or
`SkillMarkdownAdapter`.

### Publisher

A module that installs or distributes compiled artifacts. A Prompt Publisher
can detect agent hosts, install skill directories, track staleness, and remove
obsolete Prompt Artifacts. It does not compile prompts or execute Tools.

## Classification rules

Apply these questions in order:

1. Does the value describe target-neutral system meaning? It is a Capability
   fact.
2. Does it tell a model how, when, or why to behave? It belongs to a Prompt
   Guide or Prompt Artifact.
3. Can a runtime invoke it with structured arguments? It belongs to a Tool
   Contract, Tool Binding, or Tool Runtime.
4. Does it explain the system only to a person? It belongs to Documentation.
5. Does it convert or install another kind? It is an Adapter or Publisher.

If more than one answer applies, the current module is hybrid and must be
split at the relevant seam.

## Ambiguous values

| Value | Kind | Rule |
| --- | --- | --- |
| Neutral one-line summary | Capability fact | Describes what exists without directing behavior. |
| Imperative instructions | Prompt Guide | Tells a model what to do. |
| Natural-language usage example | Prompt Guide | Demonstrates model or user behavior. |
| Structured arguments and expected result | Tool Contract fixture | Verifies machine invocation. |
| Input or output JSON Schema | Tool Contract | Defines machine-readable values. |
| Destructive or idempotent annotation | Tool Contract | Affects policy and execution. |
| Handler or callback | Tool Runtime | Performs the Capability. |
| MCP server instructions | Prompt Artifact | Directs the connected model. |
| MCP tool declaration | Tool Binding | Exposes machine invocation. |
| MCP prompt | Prompt Artifact | Supplies model-directed content. |
| CLI help | Documentation Artifact | Explains commands to a person. |
| Executable JavaScript bridge | Tool Binding | Executes even though its encoding is text. |
| TypeScript declarations placed in model context (`incurs_codemode::generate_types`) | Prompt Artifact | Informs model behavior but is not the executable bridge. |
| Structured result containing text | Tool Result | Remains data until prompt compilation explicitly uses it. |

## Naming rules

Names expose the concept's kind:

| Kind | Preferred names |
| --- | --- |
| Capability | `CapabilityId`, `CapabilitySpec` |
| Prompt | `PromptGuide`, `PromptArtifact`, `PromptCompiler` |
| Tool | `ToolContract`, `ToolBinding`, `ToolRuntime`, `ToolResult` |
| Documentation | `DocumentationSource`, `DocumentationArtifact` |
| Conversion | `<Target><Kind>Adapter` |
| Publication | `<Kind>Publisher` |

Do not introduce unqualified domain names such as `Definition`, `Manifest`,
`Instructions`, `Example`, or `Description` when the kind is not clear from
the containing module.

## Architecture rules

- Prompt modules can reference `CapabilityId`; they do not receive handlers.
- Tool modules do not contain Prompt Guides, prompt programs, or few-shot
  demonstrations.
- Prompt changes do not change Tool Contract identity, policy, or digests.
- Deleting all Prompt modules leaves every Tool directly callable.
- Prompt Compilers do not execute Tools.
- Tool Runtimes do not render or publish Prompt Artifacts.
- Tool Results remain untrusted data until a Prompt module explicitly uses
  them.
- Transports can carry several kinds, but they do not merge those kinds.
- Shared source data lowers independently into Prompt, Tool, and Documentation
  representations.

## Current Incurs mapping

The following types identify existing seams and transitional hybrids. This
table describes the current architecture; the preferred terms above control
new design work.

| Existing module or type | Current classification |
| --- | --- |
| `CommandDef` | Hybrid Capability, Prompt Guide, and Tool Runtime source |
| `ToolDefinition` | Hybrid Tool Contract and Prompt Guide |
| `ToolCatalog` | Tool Contract catalog and Tool Runtime |
| `skill::CommandInfo` | Prompt Compiler input |
| `skill` | Prompt Compiler and Markdown adapter |
| `ConnectorDescription` | Hybrid Tool Contract and Prompt Guide |
| `Connector::execute` | Tool Binding execution |
| `sync_skills` | Hybrid Prompt Compiler and Prompt Publisher |
| `agents` | Host-specific Prompt publication adapters |
| `agent_plugin::AgentPluginSkillOptions` | Prompt Artifact publication input |
| `agent_plugin::AgentPluginToolBindingOptions` | Tool Binding publication input |
| `agent_plugin::publish` | Agent Plugins package Publisher |
| `McpAnnotations` | Tool Contract policy metadata |
| `McpResultContent` | Tool Result presentation metadata |

## Verification

An architecture change satisfies this taxonomy when reviewers can answer all
of these questions from names and types alone:

- Which Capability does this value describe?
- Is this content consumed by a model, a person, or a runtime?
- Can changing it affect machine invocation or only model guidance?
- Which adapter lowers it for a host or protocol?
- Can the Tool still execute when every Prompt Artifact is absent?
