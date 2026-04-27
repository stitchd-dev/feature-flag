// Placeholder screens — filled in Phase 3 onwards
import { PageHeader } from '../components/primitives'
import { I } from '../components/icons'

function Placeholder({ title, icon }: { title: string; icon?: string }) {
  const Ic = icon ? I[icon] : null
  return (
    <>
      <PageHeader
        title={title}
        crumbs={['Stitchd', title]}
        actions={<button className="btn primary"><I.plus size={14} /> New</button>}
      />
      <div className="page-body">
        <div className="empty">
          <div className="empty-icon">{Ic && <Ic size={22} stroke="var(--fg-subtle)" />}</div>
          <div className="empty-title">Coming soon</div>
          <div className="empty-desc">This screen will be implemented in the next phase.</div>
        </div>
      </div>
    </>
  )
}

export const Dashboard = () => <Placeholder title="Dashboard" icon="home" />
export const FlagsList = () => <Placeholder title="Feature Flags" icon="flag" />
export const FlagDetail = () => <Placeholder title="Flag Detail" icon="flag" />
export const SegmentsList = () => <Placeholder title="Segments" icon="segment" />
export const SegmentDetail = () => <Placeholder title="Segment Detail" icon="segment" />
export const ExperimentsList = () => <Placeholder title="Experiments" icon="beaker" />
export const ExperimentDetail = () => <Placeholder title="Experiment Detail" icon="beaker" />
export const EventsRegistry = () => <Placeholder title="Events" icon="event" />
export const Environments = () => <Placeholder title="Environments & SDK Keys" icon="key" />
export const Members = () => <Placeholder title="Members & Roles" icon="users" />
export const AuditLog = () => <Placeholder title="Audit Log" icon="history" />
export const SuperAdmin = () => <Placeholder title="Super Admin" icon="shield" />
