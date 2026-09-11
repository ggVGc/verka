defmodule Styra.MixProject do
  use Mix.Project

  # No runtime dependencies, deliberately. The protocol library is generated
  # and needs nothing; the client needs a JSON codec and a Unix socket, and
  # Elixir has carried `JSON` since 1.18 while OTP has spoken Unix sockets for
  # far longer. A project that would rather use Jason can add it and pass
  # `json: Jason`. Vendoring `lib/styra` into another project therefore drags
  # nothing along with it.
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

  # `ex_doc` is the exception, and only for `mix docs`: it is dev-only and not
  # loaded at runtime, so the library above stays dependency-free.
  #
  # It earns its place here because of what the generator goes to the trouble
  # of carrying. Every `@doc` in `protocol.ex` is the doc comment the protocol
  # author wrote in Rust, field tables and all, and without this it is only
  # ever read by whoever opens the generated file. With it, the protocol
  # documents itself in the browser, in the language the reader is working in.
  defp deps do
    [{:ex_doc, "~> 0.34", only: :dev, runtime: false}]
  end

  defp docs do
    [
      main: "readme",
      extras: ["README.md"],
      # The vocabulary first, the plumbing second, which is the order they are
      # read in.
      groups_for_modules: [
        Protocol: [~r/^Styra\.Protocol/],
        Client: [Styra.Client]
      ]
    ]
  end

  def application do
    [extra_applications: []]
  end
end
