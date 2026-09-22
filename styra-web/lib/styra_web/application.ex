defmodule StyraWeb.Application do
  # See https://elixir.hexdocs.pm/Application.html
  # for more information on OTP Applications
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      StyraWebWeb.Telemetry,
      {DNSCluster, query: Application.get_env(:styra_web, :dns_cluster_query) || :ignore},
      {Phoenix.PubSub, name: StyraWeb.PubSub},
      # Start a worker by calling: StyraWeb.Worker.start_link(arg)
      # {StyraWeb.Worker, arg},
      # Start to serve requests, typically the last entry
      StyraWebWeb.Endpoint
    ]

    # See https://elixir.hexdocs.pm/Supervisor.html
    # for other strategies and supported options
    opts = [strategy: :one_for_one, name: StyraWeb.Supervisor]
    Supervisor.start_link(children, opts)
  end

  # Tell Phoenix to update the endpoint configuration
  # whenever the application is updated.
  @impl true
  def config_change(changed, _new, removed) do
    StyraWebWeb.Endpoint.config_change(changed, removed)
    :ok
  end
end
