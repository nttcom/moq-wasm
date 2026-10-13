export async function checkCodecSupport(
  configLabel: string,
  isConfigSupported: () => Promise<{ supported?: boolean }>
): Promise<string | null> {
  try {
    const { supported } = await isConfigSupported()
    return supported ? null : `${configLabel} is not supported by this browser.`
  } catch (error) {
    console.error(`[meeting] ${configLabel} support check failed`, error)
    return `${configLabel} support check failed.`
  }
}
