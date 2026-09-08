declare module 'lucide-react/dist/esm/icons/*' {
  import type { ForwardRefExoticComponent, RefAttributes, SVGProps } from 'react'

  type IconProps = Omit<SVGProps<SVGSVGElement>, 'ref'> & {
    size?: string | number
    absoluteStrokeWidth?: boolean
  }

  const Icon: ForwardRefExoticComponent<IconProps & RefAttributes<SVGSVGElement>>
  export default Icon
}
