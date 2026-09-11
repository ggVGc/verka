defmodule Styra.MixProject do
  use Mix.Project

  def project do
    [
      app: :styra,
      version: "0.1.0",
      elixir: "~> 1.18",
      description: "An Elixir client for the Styra protocol",
      elixirc_paths: ["lib"],
      deps: deps(),
      docs: docs()
    ]
  end

  # The protocol is depended on, not copied. It lives beside the Rust
  # definitions it is generated from, which is the only place it can live
  # without there being two of it — and two of it is exactly the drift the
  # generator exists to prevent.
  #
  # What is left here is the two things the protocol deliberately does not
  # have, and neither needs a dependency either: Elixir has carried `JSON`
  # since 1.18 and OTP has spoken Unix sockets for far longer. A project that
  # would rather use Jason can add it and pass `json: Jason`.
  #
  # `ex_doc` is dev-only and not loaded at runtime, so what a release ships is
  # still only this client and the protocol under it.
  defp deps do
    [
      {:styra_protocol, path: "../styra-protocol/elixir"},
      {:ex_doc, "~> 0.34", only: :dev, runtime: false}
    ]
  end

  defp docs do
    [
      main: "readme",
      extras: ["README.md"],
      groups_for_modules: [Client: [Styra.Client]]
    ]
  end

  def application do
    [extra_applications: []]
  end
end
