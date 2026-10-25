#!/usr/bin/env bash
# Source this from a runner already positioned at its repository root.
acceptance_assert_unchanged_tree() {
  local pinned_head="$1" current_head dirty
  current_head="$(git rev-parse HEAD)" || return 1
  if [[ "$current_head" != "$pinned_head" ]]; then
    echo "acceptance provenance rejected: HEAD changed from $pinned_head to $current_head" >&2
    return 1
  fi
  dirty="$(git status --porcelain --untracked-files=all)" || return 1
  if [[ -n "$dirty" ]]; then
    echo "acceptance provenance rejected: worktree changed" >&2
    printf '%s\n' "$dirty" | head -20 >&2
    return 1
  fi
}

acceptance_validate_range() {
  local from="$1" to="$2" total="$3"
  if [[ ! "$from" =~ ^[1-9][0-9]*$ || ! "$to" =~ ^[1-9][0-9]*$ ]] \
    || [[ ${#from} -gt ${#total} || ${#to} -gt ${#total} ]] \
    || (( from > to || to > total )); then
    echo "invalid acceptance range: --from $from --to $to; require 1 <= from <= to <= $total" >&2
    return 1
  fi
}
