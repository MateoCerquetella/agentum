import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { SidebarHierarchyMock } from './SidebarHierarchyMock'
import './sidebar-hierarchy-mock.css'

const root = document.getElementById('root')
if (!root) throw new Error('Mock root element not found')

createRoot(root).render(
  <StrictMode>
    <SidebarHierarchyMock />
  </StrictMode>
)
