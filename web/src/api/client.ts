// The only way the app calls the API (ADR 0008). The browser authenticates with the session cookie alone.
import createClient from 'openapi-fetch'
import type { components, paths } from './schema'

export const api = createClient<paths>({ baseUrl: '' })

type ErrorBody = components['schemas']['ErrorBody']

/** A refused or failed request, always with an `ErrorBody`, even when the server sent none. */
export class ApiError extends Error {
  readonly status: number
  readonly body: ErrorBody

  constructor(status: number, body: ErrorBody) {
    super(body.message)
    this.status = status
    this.body = body
  }
}

function errorBody(status: number, error: unknown): ErrorBody {
  if (typeof error === 'object' && error !== null && 'code' in error && 'message' in error) {
    const { code, message } = error
    if (typeof code === 'string' && typeof message === 'string') return error as ErrorBody
  }
  return {
    code: `http_${String(status)}`,
    message: status === 0 ? 'the server did not answer' : `the server answered ${String(status)}`,
  }
}

/** The answer's data, or an `ApiError`. A network failure is status 0. */
export async function call<T>(request: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  let result: { data?: T; error?: unknown; response: Response }
  try {
    result = await request
  } catch {
    throw new ApiError(0, errorBody(0, undefined))
  }
  if (!result.response.ok) throw new ApiError(result.response.status, errorBody(result.response.status, result.error))
  return result.data as T
}
