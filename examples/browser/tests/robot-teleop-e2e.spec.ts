import { expect, type Page } from '@playwright/test'
import { type ChildProcess, spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { ROBOT_TELEOP_INDEX_PATH } from '../playwright.helpers'
import { archivedPayloads, type MemberArchive, test } from './recorder-e2e-arrange'

const moqtUrl = process.env.ROBOT_TELEOP_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const room = process.env.ROBOT_TELEOP_E2E_ROOM ?? 'anon/robot-teleop'
const policyRoom = process.env.ROBOT_TELEOP_E2E_POLICY_ROOM ?? 'anon/robot-teleop-policy'
const recordedRoom = process.env.ROBOT_TELEOP_E2E_RECORDED_ROOM ?? 'anon/robot-teleop-recorded'
const REPO_ROOT = resolve(__dirname, '..', '..', '..')
const TASK = 'go to the corner'

async function robotX(page: Page): Promise<number> {
  const text = (await page.getByTestId('robot-teleop-position').textContent()) ?? ''
  const match = /x (-?\d+\.\d+)/.exec(text)
  return match ? Number(match[1]) : Number.NaN
}

async function connect(page: Page, roomName: string): Promise<string> {
  await page.goto(`${ROBOT_TELEOP_INDEX_PATH}?${new URLSearchParams({ moqtUrl })}`)
  await page.getByTestId('robot-teleop-room').fill(roomName)
  await page.getByTestId('robot-teleop-connect').click()
  await expect(page.getByTestId('robot-teleop-status')).toHaveText('接続しました')
  await expect(page.getByTestId('robot-teleop-position')).toContainText('x ', { timeout: 15_000 })
  return (await page.getByTestId('robot-teleop-operator').textContent()) ?? ''
}

test('an operator drives the robot with a held key and the robot holds once it is released', async ({ page }) => {
  // Arrange
  const operator = await connect(page, room)
  const startX = await robotX(page)

  // Act
  await page.keyboard.down('ArrowRight')

  // Assert: the robot follows this page's actions
  await expect(page.getByTestId('robot-teleop-driver')).toHaveText(`${operator} (action)`, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(startX + 0.1)

  // Act
  await page.keyboard.up('ArrowRight')

  // Assert: without fresh actions the operator stops driving and the robot stays put
  await expect(page.getByTestId('robot-teleop-driver')).toHaveText('なし (stale)', { timeout: 10_000 })
  const stoppedX = await robotX(page)
  await page.waitForTimeout(500)
  expect(await robotX(page)).toBe(stoppedX)
})

test('an operator intervenes on the policy, which drives again once the operator stops', async ({ page }) => {
  // Arrange: the policy drives the robot towards (0.5, 0.5)
  const operator = await connect(page, policyRoom)
  const driver = page.getByTestId('robot-teleop-driver')
  await expect(driver).toHaveText(/^bots\/policy /, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(0.3)

  // Act
  await page.keyboard.down('ArrowLeft')

  // Assert
  await expect(driver).toHaveText(`${operator} (action)`, { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeLessThan(0.2)

  // Act
  await page.keyboard.up('ArrowLeft')

  // Assert
  await expect(driver).toHaveText('bots/policy (stale)', { timeout: 10_000 })
  await expect.poll(() => robotX(page), { timeout: 10_000 }).toBeGreaterThan(0.3)
})

/// Starts a robot example from `target/debug`, which the runner builds, and resolves once its stdout matches `ready`.
/// The examples dial the relay over raw QUIC on the page's port, hence `moqt:`.
async function startExample(binary: string, args: string[], ready: RegExp): Promise<ChildProcess> {
  const child = spawn(
    join(REPO_ROOT, 'target', 'debug', binary),
    ['--url', moqtUrl.replace(/^https:/, 'moqt:'), '--insecure', ...args],
    { cwd: REPO_ROOT, stdio: ['ignore', 'pipe', 'inherit'] }
  )
  await new Promise<void>((resolveReady, rejectReady) => {
    let output = ''
    child.stdout?.on('data', (chunk: Buffer) => {
      output += chunk.toString()
      if (ready.test(output)) {
        resolveReady()
      }
    })
    child.on('exit', (code) => rejectReady(new Error(`${binary} exited with ${code}`)))
  })
  return child
}

/// The `data` of every record in an event timeline track's objects.
function recordData(payloads: string[]): unknown[] {
  return payloads.flatMap((payload) => (JSON.parse(payload) as { data: unknown }[]).map((record) => record.data))
}

test('a MoQ room recording keeps the robot, the operator and the policy bot with their tracks', async ({
  recorderServe,
  page
}) => {
  // Arrange: the robot opens its episode once the room is recorded, and an operator intervenes on the policy
  const recording = await recorderServe.start(recordedRoom, 'combined', false, 'moq')
  const examples = [
    await startExample('moq-robot-sim', ['--room', recordedRoom, '--policy-episode', TASK], /robot ready/),
    await startExample('moq-robot-policy', ['--room', recordedRoom], /policy ready/)
  ]
  try {
    const operator = await connect(page, recordedRoom)
    const driver = page.getByTestId('robot-teleop-driver')
    await expect(driver).toHaveText(/^bots\/policy /, { timeout: 10_000 })
    await page.keyboard.down('ArrowLeft')
    await expect(driver).toHaveText(`${operator} (action)`, { timeout: 10_000 })
    await page.keyboard.up('ArrowLeft')
    await expect(driver).toHaveText('bots/policy (stale)', { timeout: 10_000 })

    // Act
    await recorderServe.stop(recording)

    // Assert: the robot's archive holds its state, the episode's start, and the episode's driver as its first control;
    // each driver's archive holds its actions
    const room = JSON.parse(await readFile(join(recorderServe.recordingDir(recording), 'manifest.json'), 'utf8'))
    const archiveOf = async (member: string): Promise<MemberArchive> => {
      const entry = room.members.find(
        (candidate: { namespace: string }) => candidate.namespace === `${recordedRoom}/${member}`
      )
      expect(entry?.camera.length, member).toBeGreaterThan(0)
      return JSON.parse(await readFile(join('/', entry.camera[0]), 'utf8'))
    }
    const robot = await archiveOf('robot')
    expect(recordData(await archivedPayloads(robot, 'state')).length).toBeGreaterThan(0)
    expect(recordData(await archivedPayloads(robot, 'episode'))).toEqual([{ event: 'start', id: 1, task: TASK }])
    const controls = recordData(await archivedPayloads(robot, 'control'))
    expect(controls[0]).toEqual({ driver: 'bots/policy', reason: 'episode' })
    expect(controls).toEqual(
      expect.arrayContaining([
        { driver: operator, reason: 'action' },
        { driver: 'bots/policy', reason: 'stale' }
      ])
    )
    for (const member of [operator, 'bots/policy']) {
      expect(recordData(await archivedPayloads(await archiveOf(member), 'action')).length, member).toBeGreaterThan(0)
    }
  } finally {
    for (const example of examples) {
      example.kill('SIGINT')
    }
  }
})
