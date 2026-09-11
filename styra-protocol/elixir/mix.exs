defmodule Styra.Protocol.MixProject do
  use Mix.Project

  # The generated protocol library, packaged so it can be depended on rather
  # than copied. Everything under `lib` is written by
  #
  #   cargo run -p styra-protocol --bin styra-codegen -- elixir
  #
  # and a test in the crate fails the moment it stops matching the Rust type
  # definitions it is read out of. This file and the README beside it are the
  # only hand-written things here.
  def project do
    [
      app: :styra_protocol,
      version: "0.1.0",
      elixir: "~> 1.18",
      description: "The Styra wire protocol, generated from its Rust definitions",
      elixirc_paths: ["lib"],
      deps: deps(),
      docs: docs()
    ]
  end

  def application do
    [extra_applications: []]
  end

  # No runtime dependencies: the protocol is a vocabulary and a validator, and
  # needs no codec or transport to be either.
  #
  # `ex_doc` is dev-only, and it earns its place by what the generator goes to
  # the trouble of carrying. Every `@doc` in `protocol.ex` is the doc comment
  # the protocol author wrote in Rust, field tables and all; without this it is
  # only ever read by whoever opens the generated file.
  defp deps do
    [{:ex_doc, "~> 0.34", only: :dev, runtime: false}]
  end

  defp docs do
    [
      main: "readme",
      extras: ["README.md"],
      groups_for_modules: [
        Requests: [Styra.Protocol.Request],
        Spellings: [~r/^Styra\.Protocol\./]
      ]
    ]
  end
end
