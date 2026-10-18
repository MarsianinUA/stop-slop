// @path src/orders/orders.controller.ts
// @alias @/*
import { Body, Controller, Delete, Get, Header, Param, Patch, Post, HttpCode, HttpStatus } from '@nestjs/common';
import { ApiOkResponse } from '@nestjs/swagger';

import { AdminEndpoint } from '@/http/admin-endpoint.decorator';
import { Order, OrderInput } from './order.types';

@Controller('orders')
export class OrdersController {
  @Get()
  @HttpCode(HttpStatus.OK)
  list(): Order[] {
    return [];
  }

  @Get(':id')
  @HttpCode(HttpStatus.OK)
  @ApiOkResponse({ type: Order })
  find(@Param('id') id: string): Order {
    return { id };
  }

  @Post()
  @HttpCode(HttpStatus.CREATED)
  create(@Body() input: OrderInput): Order {
    return { id: input.id };
  }

  @Header('Cache-Control', 'no-store') @Get('fresh')
  @HttpCode(HttpStatus.OK)
  fresh(): Order[] {
    return [];
  }

  @Get('recent') @HttpCode(HttpStatus.OK) @Header('Cache-Control', 'no-store')
  recent(): Order[] {
    return [];
  }

  @AdminEndpoint()
  @Post('import')
  @HttpCode(HttpStatus.CREATED)
  importAll(@Body() orders: Order[]): Order[] {
    return orders;
  }

  @Get('audit')
  @AdminEndpoint()
  @ApiOkResponse({ type: [Order] })
  @HttpCode(HttpStatus.OK)
  audit(): Order[] {
    return [];
  }

  @Get('ping') @AdminEndpoint() @HttpCode(HttpStatus.OK) ping(): string {
    return 'pong';
  }

  @Delete(':id')
  remove(@Param('id') id: string): void {
    void id;
  }

  @Patch(':id')
  update(@Param('id') id: string, @Body() input: OrderInput): Order {
    return { id: input.id ?? id };
  }

  @Get('search')
  @Post('search')
  search(@Body() input: OrderInput): Order[] {
    return [{ id: input.id }];
  }
}
