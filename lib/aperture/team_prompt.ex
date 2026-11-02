defmodule Aperture.TeamPrompt do
  @moduledoc """
  A team member's system prompt: `prompts/_team.md` filled in with their
  role, the brief the lead wrote, the roster and their workspace.
  """

  @spec render(String.t()) :: String.t()
  def render(name) do
    {team, me} = Aperture.Teams.member(name)
    template = File.read!(Path.join(Application.fetch_env!(:aperture, :root), "prompts/_team.md"))

    vars = %{
      "name" => me["name"],
      "role" => me["role"],
      "team" => team["name"],
      "lead" => team["lead"],
      "brief" => me["brief"],
      "roster" => roster(team, me),
      "workspace" => workspace(team, me)
    }

    # one pass, so text inside the brief is never treated as a placeholder
    Regex.replace(~r/\{\{(\w+)\}\}/, template, fn whole, key -> Map.get(vars, key, whole) end)
  end

  defp roster(team, me) do
    lead = "- `#{team["lead"]}`: lead (assigns the work, reviews it, talks to Francisco)"

    members =
      for m <- team["members"] do
        you = if m["name"] == me["name"], do: " (you)", else: ""
        "- `#{m["name"]}`#{you}: #{m["role"]}, on #{m["harness"]} / #{m["model"]}"
      end

    Enum.join([lead | members], "\n")
  end

  defp workspace(team, %{"branch" => branch} = me) do
    "Your own git worktree of `#{team["project"]}`: `#{me["cwd"]}`, on branch " <>
      "`#{branch}` (made from `#{team["base"]}`). Commit your work there; the branch " <>
      "is kept when the team is dissolved, so committed work is never lost."
  end

  defp workspace(team, _me) do
    "The project directory `#{team["project"]}`, shared with the team. Unless your " <>
      "brief says otherwise you read and run things here but don't edit files: members " <>
      "who write code have their own worktrees."
  end
end
