'use client'

import type { ImportResult } from '../schemas'
import { fetchApi } from '../fetch'

export type NativeImportMode = 'replace' | 'append'
export type NativeImportSource = 'files' | 'folder'

export const importDocumentsFromDialog = async (
  mode: NativeImportMode,
  source: NativeImportSource,
): Promise<ImportResult> => {
  const params = new URLSearchParams({ mode, source })
  return fetchApi<ImportResult>(`/api/v1/documents/import-dialog?${params}`, {
    method: 'POST',
  })
}
