defmodule NotaWeb.Application do
  # See https://elixir.hexdocs.pm/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      NotaWebWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:nota_web, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: NotaWeb.PubSub},
      # Start a worker by calling: NotaWeb.Worker.start_link(arg)
      # {NotaWeb.Worker, arg},
      # Start to serve requests, typically the last entry
      NotaWebWeb.Endpoint
    ]

    # See https://elixir.hexdocs.pm/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: NotaWeb.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    NotaWebWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
