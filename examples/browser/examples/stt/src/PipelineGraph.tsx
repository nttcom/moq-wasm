import { Background, Controls, Handle, Position, ReactFlow, type Edge, type Node } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import type { StageId, Topology, Turn } from './types'

const NODE_SPACING_X = 210
const NODE_SPACING_Y = 150
const ROW_LENGTH = 4

type NodeState = 'idle' | 'running' | 'done' | 'failed'

type StageNodeData = {
  label: string
  kind: 'client' | 'track' | 'stage'
  impl?: string
  state: NodeState
  detail: string
}

type LiveState = { state: NodeState; detail: string }

const STATE_COLORS: Record<NodeState, string> = {
  idle: '#cbd5e1',
  running: '#f59e0b',
  done: '#22c55e',
  failed: '#ef4444'
}

function StageNode({ data }: { data: StageNodeData }) {
  return (
    <div
      style={{
        minWidth: 150,
        padding: '10px 12px',
        borderRadius: 10,
        border: `2px solid ${STATE_COLORS[data.state]}`,
        background: data.kind === 'stage' ? '#0f172a' : '#1e293b',
        color: '#e2e8f0',
        boxShadow: data.state === 'running' ? `0 0 12px ${STATE_COLORS.running}` : 'none'
      }}
    >
      <Handle type="target" position={Position.Left} />
      <div style={{ fontSize: 13, fontWeight: 600 }}>{data.label}</div>
      {data.impl && <div style={{ fontSize: 11, opacity: 0.7 }}>{data.impl}</div>}
      <div style={{ fontSize: 11, marginTop: 4, color: STATE_COLORS[data.state] }}>{data.detail}</div>
      <Handle type="source" position={Position.Right} />
    </div>
  )
}

const nodeTypes = { stage: StageNode }

function stageState(id: string, turn: Turn | undefined): LiveState {
  const stage = id as StageId
  if (turn?.failed === stage) {
    return { state: 'failed', detail: 'failed' }
  }
  if (turn?.running === stage) {
    return { state: 'running', detail: 'running…' }
  }
  const elapsed = turn?.stages[stage]
  if (elapsed !== undefined) {
    return { state: 'done', detail: `${Math.round(elapsed)} ms` }
  }
  return { state: 'idle', detail: 'idle' }
}

function endpointState(id: string, turn: Turn | undefined, audioObjects: number): LiveState {
  if (id === 'mic' || id === 'audio') {
    return {
      state: audioObjects > 0 ? 'running' : 'idle',
      detail: id === 'mic' ? `${audioObjects} objects sent` : 'publishing'
    }
  }
  if (id === 'reply') {
    return turn?.replyPackets
      ? { state: 'done', detail: `${turn.replyPackets} packets` }
      : { state: 'idle', detail: 'idle' }
  }
  if (turn?.totalMs !== undefined) {
    return { state: 'done', detail: `turn ${turn.turn}: ${Math.round(turn.totalMs)} ms` }
  }
  return turn?.replyPackets ? { state: 'running', detail: 'playing…' } : { state: 'idle', detail: 'idle' }
}

export function PipelineGraph({
  topology,
  turn,
  audioObjects
}: {
  topology: Topology | null
  turn: Turn | undefined
  audioObjects: number
}) {
  if (!topology) {
    return <p style={{ color: '#94a3b8', fontSize: 13 }}>Waiting for the pipeline topology from the server…</p>
  }
  const nodes: Node<StageNodeData>[] = topology.nodes.map((node, index) => ({
    id: node.id,
    type: 'stage',
    position: {
      x: (index % ROW_LENGTH) * NODE_SPACING_X,
      y: Math.floor(index / ROW_LENGTH) * NODE_SPACING_Y
    },
    data: {
      label: node.label,
      kind: node.kind,
      impl: node.impl,
      ...(node.kind === 'stage' ? stageState(node.id, turn) : endpointState(node.id, turn, audioObjects))
    }
  }))
  const edges: Edge[] = topology.edges.map(([source, target]) => ({
    id: `${source}-${target}`,
    source,
    target,
    animated: nodes.find((node) => node.id === target)?.data.state === 'running',
    style: { stroke: '#475569' }
  }))
  return (
    <div style={{ height: 380, border: '1px solid #334155', borderRadius: 10 }}>
      <ReactFlow nodes={nodes} edges={edges} nodeTypes={nodeTypes} fitView proOptions={{ hideAttribution: true }}>
        <Background color="#334155" gap={18} />
        <Controls showInteractive={false} />
      </ReactFlow>
    </div>
  )
}
