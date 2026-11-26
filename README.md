dual purpose
  - platform for experimenation around LLM-assisted development
  - experiment in vibe-slopping while trying to create something:
    - constraints, isolation, modularization
    - Not going super well so far. Started paying the price, and develoment slows down

Current:
  - Styra:
    - server-client
    - context management and interaction branching
    - worktree association and management, integration and automatic conflict resolution
    - sandboxing (bwrap), for safety and context control:
      - All accessible data is potential context for the agent

  - styra-diff (rename)
  - Driva
  - Genta
  - Nota


Explain how things fit together

Styra plans:
  - DSL:
    - 


Background:

  Started as attempt of creating a graph-based tool for LLM work, with verifiable tracking of all inputs and outputs.
  Lead to styra, from observability need.
  Realised I need to develop the workflow and understand the necessary building blocks interactively first. Hence starting from "custom coding harness" perspective instead:
    - Driva
    - Genta
    - Styra server + client/tui

  Pet peeves in existing harnesses:
    - Overwhelming river of text and diffs flying past with no easy retroactive inspection
    - Manual management of the relationsip between worktrees and agent interactions
    - Lacking view of changes made by agent, in total or per turn
    - Cumbersome to reference specific changes or prior history in 
    - Sanboxing I do not trust

Currently approaching some kind of runtime which could now be used as the execution environment automatic work by LLM tools.
Still need task/work graph


Future:
  - Linka:
    - Should only track hashes and links, and supply tool for atomically updating the graph.
    - Does not care where content is stored. Actual task descriptions and relationships can be supplied by different tool, as long as linka can see the stored data.
  - Orka (maybe?)
  - Custom harness for total tool control using API access, for available providers (currently only OpanAI)



General:
  - Give agents as much freedom as possible, while maintaining external control and expectations:
    - Reasoning: Setting up an environment to work freely in seems like the only realistic option, compared to precristive rules and requests.
    - The more we try to figure out what the LLM will do beforehand and control for it, the more effort we need to put in, meaning we get less value out of it.
