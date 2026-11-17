// Prints the section of the version given from the CHANGELOG.md given, the body of its GitHub Release.
import { readFileSync } from 'node:fs'

const [changelogPath, version] = process.argv.slice(2)
const changelog = readFileSync(changelogPath, 'utf8')
const sections = changelog.split(/^## /m).slice(1)
const section = sections.find((text) => text.startsWith(`${version}\n`))
if (!section) {
  console.error(`${changelogPath} has no section for ${version}`)
  process.exit(1)
}
process.stdout.write(section.slice(section.indexOf('\n') + 1).trim() + '\n')
