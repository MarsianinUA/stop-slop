// @path src/orders/dto/order.response.dto.ts
import { ApiProperty, ApiPropertyOptional } from '@nestjs/swagger';
import { ApiPropertyOptional as Optional } from '@nestjs/swagger';

export class OrderItemDto {
  @ApiPropertyOptional({ type: String }) // expect: no-api-property-optional
  note?: string;

  @ApiProperty({ type: Number, required: false }) // expect: no-api-property-optional
  discount?: number;

  @Optional() // expect: no-api-property-optional
  coupon?: string | null;
}

export class OrderResponseDto {
  @ApiProperty()
  id!: string;

  @ApiPropertyOptional({ // expect: no-api-property-optional
    type: () => [OrderItemDto],
  })
  items?: OrderItemDto[];

  @ApiPropertyOptional() @ApiProperty({ nullable: true }) // expect: no-api-property-optional
  ['shipped-at']?: string | null;
}
