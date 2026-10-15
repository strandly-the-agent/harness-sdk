/** SDK languages the docs document. Lowercase slugs, as used in the `languages` frontmatter field. */
export const ALL_SDK_LANGUAGES = ['python', 'typescript', 'rust'] as const
export type SdkLanguage = (typeof ALL_SDK_LANGUAGES)[number]
