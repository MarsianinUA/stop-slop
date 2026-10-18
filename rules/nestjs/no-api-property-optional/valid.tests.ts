// @path src/__tests__/order.response.spec.ts
import { ApiPropertyOptional } from '@nestjs/swagger';

export class FakeOrderResponseDto {
  @ApiPropertyOptional()
  note?: string;
}
