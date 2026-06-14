# llaundry
Hierarchical LLM coding


- All edits and interaction with project are done through MCP which understands the hierarchical layout
- Structure is based on a tree of:
  - Descriptions
  - Task lists, associated with descriptions
  - Implementations, associated with tasks
  - Executable artifacts, associated with implementation sources
  - Tests/verifications, associated with implementation sources or produced artifacts:
    - One verification can be associated with multiple inputs
  - Multiple tasks can be inputs to a single implementation node
  - Multiple implementation nodes can be the input of one build node
  - Some implementations may rely on the artifact output of another task node:
    - For example, a tool may need to be created (implemented and built), which another task will use to process data, in order to complete its implementation work
-  An important part of the system is the the ability to track which parts of the specification and implementaion of the system changes, relative to which verifications change.


# Further improvements:
- Each node should store a hash of all the files and artifacts it produces, and there should be some mechanism of verifying that only the referenced files are used by other nodes.

# Example session
- User makes a request to implement a feature
- There is some back and forth with the LLM agent.
- The LLM agent produces a suggested list of tasks
- The user accepts the planned work.
- The agent uses the llaundry MCP to store the initial request and creates a task node for each planned task, connected to the initial request node:
  - If the tasks are dependent on each-other, that should be indicated by connections, otherwise they are assumed to be parallelisable.
- The planning agent finished, and a new implementor agent is launched. It is handed the ID of the description node, and proceeds to work on each associated task:
  - Implementation consists of producing code in Go
  - When implementation is finished, metadata about which files were produced is added to the implementation node and it is marked completed.
  - A verification node is created, pointing to the implementation node. The implementation agent is closed and a verification agent is launched. The verification agent uses the llaundry MCP to run verification on the generated sources:
    - If successful, the verification node is marked as complete, storing the output returned from the verification step in the llaundry MCP.
    - If failed, the implementation agent double checks that the test is correct, based on the task nodes related to the associated implementation node. If the tests are valid, but verification still fails, the output from verification step in MCP server is stored in the verification node and the associated implementation node is marked as failed. The verification agent is closed. And a new implementation agent is launched to fix the implementation, based on the metadata in verification node, and the associated tasks.
    - Repeat until verification succeeds 
  - When implementation is verified, a build node is created, pointing to the implementation node.
- A builder agent is launched to process the build node:
  - It should use the llaundry MCP to create an artifact based on the metadata in the implementation node
  - If build succeeds, the build node is marked as completed, and the main artifact as well as any necessary metadata about using it is added to the node.
  - If the build fails, information about the failure is added to the metadata, and the node is marked as failed.
