import { LinearIcon } from '@/components/icons/LinearIcon'
import React from 'react'
import { Github, Gitlab } from 'lucide-react'
import type { TaskProvider } from '../../../shared/types'

export type TaskSource = TaskProvider

type SourceOption = {
  id: TaskSource
  label: string
  Icon: (props: { className?: string }) => React.JSX.Element
  disabled?: boolean
}

export const SOURCE_OPTIONS: SourceOption[] = [
  {
    id: 'github',
    label: 'GitHub',
    Icon: ({ className }) => <Github className={className} />
  },
  {
    id: 'gitlab',
    label: 'GitLab',
    Icon: ({ className }) => <Gitlab className={className} />
  },
  {
    id: 'linear',
    label: 'Linear',
    Icon: ({ className }) => <LinearIcon className={className} />
  }
]
