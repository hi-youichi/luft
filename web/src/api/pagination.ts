export type Cursor = string | null

export interface OffsetPagination {
  page: number
  page_size: number
}

export interface CursorPagination {
  cursor: Cursor
  limit: number
}

export type PaginationParams = OffsetPagination | CursorPagination

export interface PageMeta {
  page: number
  page_size: number
  total: number
  total_pages: number
  has_next: boolean
  has_prev: boolean
}

export interface CursorMeta {
  cursor: Cursor
  next_cursor: Cursor
  has_next: boolean
  limit: number
}

export type PaginationMeta = PageMeta | CursorMeta

export interface PaginatedResponse<T> {
  items: T[]
  meta: PaginationMeta
}

export interface PaginatedRunsResponse {
  runs: PaginatedResponse<{
    run_id: string
    run_dir: string
    task: string
    status: string
    current_phase: number
    total_phases: number
    total_tokens: number
    started_at: string
    elapsed_ms: number
  }>['items']
  meta: PaginationMeta
}

export function isOffsetPagination(p: PaginationParams): p is OffsetPagination {
  return 'page' in p
}

export function isCursorPagination(p: PaginationParams): p is CursorPagination {
  return 'cursor' in p
}

export function isPageMeta(m: PaginationMeta): m is PageMeta {
  return 'total_pages' in m
}

export function isCursorMeta(m: PaginationMeta): m is CursorMeta {
  return 'next_cursor' in m
}

export function defaultOffsetPagination(page = 1, pageSize = 20): OffsetPagination {
  return { page, page_size: pageSize }
}

export function defaultCursorPagination(limit = 20): CursorPagination {
  return { cursor: null, limit }
}

export function totalPages(total: number, pageSize: number): number {
  return Math.ceil(total / pageSize) || 1
}

export function hasNextPage(page: number, total: number, pageSize: number): boolean {
  return page < totalPages(total, pageSize)
}

export function hasPrevPage(page: number): boolean {
  return page > 1
}

export function buildPageMeta(total: number, page: number, pageSize: number): PageMeta {
  return {
    page,
    page_size: pageSize,
    total,
    total_pages: totalPages(total, pageSize),
    has_next: hasNextPage(page, total, pageSize),
    has_prev: hasPrevPage(page),
  }
}

export function buildCursorMeta(
  nextCursor: Cursor,
  limit: number,
  hasNext: boolean,
): CursorMeta {
  return {
    cursor: null,
    next_cursor: nextCursor,
    has_next: hasNext,
    limit,
  }
}
