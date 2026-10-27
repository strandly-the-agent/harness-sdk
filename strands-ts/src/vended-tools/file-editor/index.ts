/**
 * File editor tool for programmatic filesystem interaction.
 *
 * @deprecated `fileEditor` is deprecated in favor of the focused `read`, `write`, and `edit`
 * vended tools. It will be removed in a future release.
 */

export { fileEditor, makeFileEditor, DEFAULT_FILE_EDITOR_DESCRIPTION } from './file-editor.js'
export type { MakeFileEditorOptions } from './file-editor.js'
export type { FileEditorInput, FileEditorOptions, IFileReader } from './types.js'
