export const OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT =
  'agentum:operational-sidebar-search-focus'

export function requestOperationalSidebarSearchFocus(): void {
  window.dispatchEvent(new CustomEvent(OPERATIONAL_SIDEBAR_SEARCH_FOCUS_EVENT))
}

export function focusOperationalSidebarSearch(input: HTMLInputElement | null): boolean {
  if (!input) return false
  input.focus()
  input.select()
  return true
}
