defmodule Styra.MixProject do
  use Mix.Project

  # No dependencies, deliberately. The protocol library is generated and needs
  # nothing; the client needs a JSON codec and a Unix socket, and Elixir has
  # carried `JSON` since 1.18 while OTP has spoken Unix sockets for far longer.
  # A project that would rather use Jason can add it and pass `json: Jason`.
  def project do
    [
      app: :styra,
      version: "0.1.0",
      elixir: "~> 1.18",
      description: "An Elixir client for the Styra protocol",
      elixirc_paths: ["lib"],
      deps: [],
      docs: [main: "Styra.Protocol"]
    ]
  end

  def application do
    [extra_applications: []]
  end
end
